//! Safe wrapper around [`sys`], plus probe/device helpers.
//!
//! All J-Link DLL calls are funneled through [`JLinkDll`]. The DLL is a
//! process-wide singleton and is not re-entrant, so callers must serialize
//! access (the app holds a single lock around every call).

pub mod sys;

use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::path::Path;
use std::time::{Duration, Instant};

pub use sys::{JLINKARM_TIF_JTAG, JLINKARM_TIF_SWD};

/// One discovered J-Link probe.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    pub serial: u32,
    pub product: String,
    pub nickname: String,
    pub firmware: String,
    /// Raw `JLINKARM_HOSTIF_*` value: 1 = USB, 2 = IP.
    pub connection: u32,
}

/// One RTT buffer as reported by the DLL.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RttBufferInfo {
    pub index: i32,
    pub direction: u32,
    pub name: String,
    pub size: u32,
}

/// One target device from the DLL's built-in database.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub vendor: String,
    pub name: String,
    /// Human-readable core name from `JLINKARM_Core2CoreName`, e.g. `Cortex-M4`.
    pub core: String,
    pub ram_bytes: u64,
    pub flash_bytes: u64,
}

/// Upper bound on how many device indices to probe before giving up. Current
/// J-Link DLLs know well under 20k devices; this only guards against a DLL that
/// never reports an error.
const MAX_DEVICE_INDEX: usize = 100_000;

/// A loaded `JLink_x64.dll` with resolved entry points.
pub struct JLinkDll {
    _lib: Library,
    api: sys::RawApi,
    pub path: std::path::PathBuf,
}

impl JLinkDll {
    /// Load the DLL from an absolute path and resolve required symbols.
    pub fn load(path: &Path) -> Result<Self> {
        let lib = unsafe { Library::new(path) }
            .with_context(|| format!("无法加载 J-Link DLL: {}", path.display()))?;
        let api = unsafe { resolve(&lib) }
            .with_context(|| format!("DLL 缺少必需的导出函数: {}", path.display()))?;
        let dll = Self { _lib: lib, api, path: path.to_path_buf() };
        // Must happen before any operation that can raise a dialog (including
        // probe enumeration and Open). See `suppress_gui`.
        dll.suppress_gui();
        Ok(dll)
    }

    /// Ask the DLL to stay headless.
    ///
    /// Without this, the DLL spawns `JLinkGUIServer.exe`, which pops an
    /// "Emulator selection" dialog whenever it is asked to enumerate probes and
    /// finds none — exactly the case where the user has not plugged the J-Link
    /// in yet. It steals focus and blocks, which is unacceptable for an embedded
    /// tool. SEGGER documents `SuppressGUI` as suppressing *all* of the DLL's
    /// GUI (message boxes and firmware dialogs included) and requires it to be
    /// set before `JLINKARM_Open`; `HideDeviceSelection` and
    /// `SuppressControlPanel` cover the device picker and control panel, and
    /// `SuppressEmuUSBDialog` is the emulator-selection dialog itself.
    ///
    /// Probe selection is deliberately left to this application's own UI.
    pub fn suppress_gui(&self) {
        for cmd in [
            "SuppressGUI 1",
            "SuppressEmuUSBDialog 1",
            "HideDeviceSelection 1",
            "SuppressControlPanel",
        ] {
            // Best effort: older DLLs do not know every command.
            let _ = self.exec_command(cmd);
        }
    }

    pub fn version(&self) -> u32 {
        unsafe { (self.api.get_dll_version)() }
    }

    /// Serial number of the currently opened probe (0 if no session is open).
    ///
    /// `JLINKARM_Open` picks a probe when none was explicitly selected, so this is
    /// the authoritative "which probe am I talking to" answer.
    pub fn serial_number(&self) -> u32 {
        unsafe { (self.api.get_sn)() }
    }

    /// J-Link encodes the DLL version as `major*10000 + minor*100 + patch`,
    /// where a non-zero patch is a letter suffix (`1` → `a`).
    pub fn version_string(&self) -> String {
        let v = self.version();
        let major = v / 10000;
        let minor = (v / 100) % 100;
        let patch = v % 100;
        if patch == 0 {
            format!("V{major}.{minor:02}")
        } else {
            let letter = (b'a' + ((patch - 1) % 26) as u8) as char;
            format!("V{major}.{minor:02}{letter}")
        }
    }

    /// Enumerate connected probes. Works before [`Self::open`].
    pub fn list_probes(&self) -> Result<Vec<ProbeInfo>> {
        let hostifs = sys::JLINKARM_HOSTIF_USB | sys::JLINKARM_HOSTIF_IP;
        unsafe {
            // First call with a null buffer returns the count (negative = error).
            let count = (self.api.emu_get_list)(hostifs, std::ptr::null_mut(), 0);
            if count < 0 {
                bail!("枚举调试器失败，错误码 {}", count);
            }
            if count == 0 {
                return Ok(Vec::new());
            }
            let mut infos = vec![sys::EmuConnectInfo::default(); count as usize];
            let got = (self.api.emu_get_list)(hostifs, infos.as_mut_ptr(), count);
            if got < 0 {
                bail!("枚举调试器失败，错误码 {}", got);
            }
            infos.truncate(got as usize);
            Ok(infos
                .iter()
                .map(|i| ProbeInfo {
                    serial: i.SerialNumber,
                    product: c_char_array_to_string(&i.acProduct),
                    nickname: c_char_array_to_string(&i.acNickName),
                    firmware: c_char_array_to_string(&i.acFWString),
                    connection: i.Connection,
                })
                .collect())
        }
    }

    /// Number of connected probes (U32, never negative).
    #[allow(dead_code)]
    pub fn num_probes(&self) -> u32 {
        unsafe { (self.api.emu_get_num_devices)() }
    }

    pub fn select_by_serial(&self, sn: u32) -> Result<()> {
        let r = unsafe { (self.api.emu_select_by_usbsn)(sn) };
        if r < 0 {
            // Diagnostics must stay passive. An earlier version called
            // `SelectByIndex` here to compare selection paths — a state-changing
            // call inside an error path, which multiplied DLL calls and could feed
            // the "no device attached" state that makes J-Link raise its emulator
            // selection dialog. Only read-only state is gathered now.
            let probes = self.list_probes().unwrap_or_default();
            let visible = probes
                .iter()
                .map(|x| format!("{} ({})", x.serial, x.product))
                .collect::<Vec<_>>()
                .join(", ");

            eprintln!(
                "[jlink] SelectByUSBSN({sn}) 失败 rc={r}\n\
                 [jlink]   IsOpen={} IsConnected={}\n\
                 [jlink]   DLL 可见调试器: [{visible}]\n\
                 [jlink]   传入序列号在列表中: {}",
                self.is_open(),
                self.is_connected(),
                probes.iter().any(|x| x.serial == sn),
            );

            bail!("选择调试器序列号 {sn} 失败（错误码 {r}，详见控制台 [jlink] 诊断）");
        }
        Ok(())
    }

    /// Select a probe by its 0-based index. The SDK returns the previously
    /// selected index (always a `U32`, so there is no error value to check).
    pub fn select_by_index(&self, index: u32) -> Result<()> {
        if index >= self.num_probes() {
            bail!("调试器索引 {} 超出范围（共 {} 个）", index, self.num_probes());
        }
        unsafe { (self.api.emu_select_by_index)(index) };
        Ok(())
    }

    /// `JLINKARM_Open` — success is a NULL string pointer.
    pub fn open(&self) -> Result<()> {
        let p = unsafe { (self.api.open)() };
        if !p.is_null() {
            let msg = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
            bail!("打开 J-Link 失败: {}", msg);
        }
        Ok(())
    }

    pub fn close(&self) {
        unsafe { (self.api.close)() }
    }

    pub fn set_interface(&self, tif: i32) -> Result<()> {
        let r = unsafe { (self.api.tif_select)(tif) };
        if r < 0 {
            bail!("设置目标接口失败，错误码 {}", r);
        }
        Ok(())
    }

    pub fn set_speed(&self, khz: u32) {
        unsafe { (self.api.set_speed)(khz) }
    }

    /// Run a J-Link command string (e.g. `device = STM32F407VG`).
    pub fn exec_command(&self, cmd: &str) -> Result<String> {
        let c = CString::new(cmd).context("命令包含非法字符")?;
        let mut out = vec![0 as c_char; 256];
        let r = unsafe { (self.api.exec_command)(c.as_ptr(), out.as_mut_ptr(), out.len() as i32) };
        let text = c_char_slice_to_string(&out);
        if r < 0 {
            bail!("J-Link 命令 '{}' 失败（{}）: {}", cmd, r, text);
        }
        Ok(text)
    }

    pub fn device_index(&self, name: &str) -> Result<i32> {
        let c = CString::new(name).context("芯片名包含非法字符")?;
        Ok(unsafe { (self.api.device_get_index)(c.as_ptr()) })
    }

    /// Enumerate the DLL's built-in device database.
    ///
    /// There is no "device count" function: the database is walked by index
    /// until `JLINKARM_DEVICE_GetInfo` reports failure. On a typical install this
    /// yields several thousand entries and completes in a few tens of
    /// milliseconds. No probe connection is required, so the device picker works
    /// before anything is plugged in.
    pub fn list_devices(&self) -> Vec<DeviceEntry> {
        let mut out = Vec::new();
        for i in 0..MAX_DEVICE_INDEX {
            let mut info = sys::JlinkDeviceInfo::zeroed();
            info.SizeOfStruct = std::mem::size_of::<sys::JlinkDeviceInfo>() as u32;
            let r = unsafe { (self.api.device_get_info)(i as c_int, &mut info) };
            if r < 0 || info.sName.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr(info.sName) }.to_string_lossy().into_owned();
            if name.is_empty() {
                break;
            }
            let vendor = if info.sManu.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(info.sManu) }.to_string_lossy().into_owned()
            };
            let core = self.core_name(info.Core);
            out.push(DeviceEntry {
                vendor,
                name,
                core,
                ram_bytes: info.RAMSize as u64,
                flash_bytes: info.FlashSize as u64,
            });
        }
        out
    }

    /// Map a `JLINKARM_CORE_*` value to its display name (e.g. `Cortex-M4`).
    fn core_name(&self, core: u32) -> String {
        let mut buf = [0 as c_char; 64];
        unsafe {
            (self.api.core_to_name)(core, buf.as_mut_ptr(), buf.len() as c_uint);
        }
        c_char_array_to_string(&buf)
    }

    pub fn connect(&self) -> Result<()> {
        let r = unsafe { (self.api.connect)() };
        if r < 0 {
            bail!("{}", friendly_connect_error(r));
        }
        Ok(())
    }

    pub fn is_connected(&self) -> bool {
        unsafe { (self.api.is_connected)() != 0 }
    }

    /// Whether the DLL currently has an open session.
    ///
    /// `JLINKARM_EMU_SelectByUSBSN` fails with -1 while this is true, so the
    /// connect sequence must never select a probe on an already-open DLL.
    pub fn is_open(&self) -> bool {
        unsafe { (self.api.is_open)() != 0 }
    }

    /// Whether the target CPU is halted. `JLINKARM_Connect` leaves the core
    /// halted, which stops the firmware from printing and makes RTT look dead.
    pub fn is_halted(&self) -> bool {
        unsafe { (self.api.is_halted)() != 0 }
    }

    /// Resume the target CPU.
    pub fn go(&self) {
        unsafe { (self.api.go)() }
    }

    /// Stop the target CPU.
    pub fn halt(&self) {
        unsafe { (self.api.halt)() }
    }

    /// Reset the target and leave the CPU **halted** at the reset vector.
    ///
    /// This is the state a flasher leaves behind when "reset and run" is not
    /// selected. It is used to reproduce that state in diagnostics; the app's
    /// recovery path uses [`Self::reset_no_halt`] instead.
    pub fn reset_halt(&self) -> i32 {
        unsafe { (self.api.reset)() }
    }

    /// Reset the target and leave it running.
    ///
    /// Uses `JLINKARM_ResetNoHalt` rather than `JLINKARM_Reset` because the
    /// latter halts the core: the firmware would stop immediately after reset and
    /// no RTT output would follow. After this the target is executing again, so
    /// RTT resumes on its own once the firmware re-initialises it.
    pub fn reset_no_halt(&self) {
        unsafe { (self.api.reset_no_halt)() }
    }

    /// Issue `JLINK_RTTERMINAL_Control(START)` once and return the raw result.
    ///
    /// START kicks off a search for the RTT control block in target RAM. It must
    /// be issued **once**; re-issuing it restarts that search, so a retry loop
    /// that calls START every iteration keeps aborting its own search and never
    /// converges.
    pub fn rtt_start_once(&self) -> i32 {
        let mut start = sys::RttTerminalStart::default();
        unsafe { (self.api.rtt_control)(sys::RTTERMINAL_CMD_START, &mut start as *mut _ as *mut c_void) }
    }

    /// `Some(true/false)` when the DLL reports RTT status, `None` on error.
    pub fn rtt_is_running(&self) -> Option<bool> {
        let mut status = sys::RttTerminalStatus::default();
        let r = unsafe {
            (self.api.rtt_control)(
                sys::RTTERMINAL_CMD_GETSTAT,
                &mut status as *mut _ as *mut c_void,
            )
        };
        if r >= 0 {
            Some(status.IsRunning != 0)
        } else {
            None
        }
    }

    /// Start RTT and wait up to `timeout` for the control block to be found.
    ///
    /// START begins a search of target RAM for the RTT control block, so it is
    /// issued **once**; the wait then polls for readiness. Two traps, both
    /// confirmed against the real DLL (V6.88a):
    ///
    /// * `GETSTAT.IsRunning` is **1 immediately**, even while the search is still
    ///   running and no buffers exist yet. It is therefore useless as a success
    ///   signal — waiting on it returns "connected" before RTT works.
    /// * The reliable signal is the **up-buffer count** (`GETNUMBUF`), which
    ///   converges to the real number only once the control block is found
    ///   (measured: 0 at t=0, 3 by ~500 ms on an already-running target).
    ///
    /// Re-issuing START in the wait loop restarts the search and can prevent it
    /// from ever converging, so it is deliberately not retried here.
    pub fn rtt_start(&self, timeout: Duration) -> Result<()> {
        self.rtt_start_cancellable(timeout, &|| false)
    }

    /// As [`Self::rtt_start`], but gives up early when `cancelled` returns true.
    ///
    /// The wait can take the full timeout, and the caller (the acquisition thread)
    /// has to be able to stop promptly so the process can exit instead of hanging
    /// in the background.
    pub fn rtt_start_cancellable(
        &self,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let t0 = Instant::now();
        self.rtt_start_once();

        let deadline = t0 + timeout;
        loop {
            // Up-buffer count from the dedicated command.
            if let Some(n) = self.rtt_num_up_buffers() {
                if n > 0 {
                    return Ok(());
                }
            }
            // Fall back to the same field carried in the status struct, since
            // some DLL builds only populate it there.
            if let Some(s) = self.rtt_status() {
                if s.NumUpBuffers > 0 {
                    return Ok(());
                }
            }
            if cancelled() {
                bail!("RTT 控制块搜索已取消（程序正在退出）");
            }
            if Instant::now() >= deadline {
                bail!(
                    "RTT 控制块搜索超时（{} 秒）",
                    timeout.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn rtt_stop(&self) {
        let mut stop = sys::RttTerminalStop::default();
        unsafe {
            (self.api.rtt_control)(sys::RTTERMINAL_CMD_STOP, &mut stop as *mut _ as *mut c_void);
        }
    }

    /// Number of RTT **up** buffers configured on the target.
    ///
    /// `JLINK_RTTERMINAL_Control(GETNUMBUF, &direction)` returns the count for
    /// the given direction. Requires RTT to be running (`rtt_start` succeeded).
    pub fn rtt_num_up_buffers(&self) -> Option<u32> {
        let mut dir: c_int = sys::RTTERMINAL_DIR_UP as c_int;
        let r = unsafe {
            (self.api.rtt_control)(
                sys::RTTERMINAL_CMD_GETNUMBUF,
                &mut dir as *mut _ as *mut c_void,
            )
        };
        if r >= 0 {
            Some(r as u32)
        } else {
            None
        }
    }

    /// Enumerate the RTT **up** buffers.
    ///
    /// `GETDESC` needs both the index and the direction set on the descriptor;
    /// omitting `Direction` happens to work only because UP is 0. The count comes
    /// from `rtt_num_up_buffers`, so this does not depend on an error to stop.
    pub fn rtt_up_buffers(&self) -> Vec<RttBufferInfo> {
        let count = match self.rtt_num_up_buffers() {
            Some(n) => n,
            None => return Vec::new(),
        };
        let mut out = Vec::new();
        for i in 0..count {
            let mut d = sys::RttBufDesc {
                BufferIndex: i as c_int,
                Direction: sys::RTTERMINAL_DIR_UP,
                ..Default::default()
            };
            let r = unsafe {
                (self.api.rtt_control)(
                    sys::RTTERMINAL_CMD_GETDESC,
                    &mut d as *mut _ as *mut c_void,
                )
            };
            if r < 0 {
                break;
            }
            out.push(RttBufferInfo {
                index: d.BufferIndex,
                direction: d.Direction,
                name: c_char_array_to_string(&d.acName),
                size: d.SizeOfBuffer,
            });
        }
        out
    }

    pub fn rtt_status(&self) -> Option<sys::RttTerminalStatus> {
        let mut status = sys::RttTerminalStatus::default();
        let r = unsafe {
            (self.api.rtt_control)(
                sys::RTTERMINAL_CMD_GETSTAT,
                &mut status as *mut _ as *mut c_void,
            )
        };
        if r >= 0 {
            Some(status)
        } else {
            None
        }
    }

    /// Read from an RTT up buffer. Returns bytes read (may be 0).
    pub fn rtt_read(&self, buffer_index: u32, buf: &mut [u8]) -> Result<usize> {
        let n = unsafe {
            (self.api.rtt_read)(
                buffer_index,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as u32,
            )
        };
        if n < 0 {
            bail!("RTT 读取失败，错误码 {}", n);
        }
        Ok(n as usize)
    }
}

unsafe fn resolve(lib: &Library) -> Result<sys::RawApi> {
    unsafe fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T> {
        let s = lib
            .get::<T>(name.as_bytes())
            .with_context(|| format!("缺少导出函数 {}", name))?;
        Ok(*s)
    }
    Ok(sys::RawApi {
        open: sym(lib, "JLINKARM_Open")?,
        open_ex: sym(lib, "JLINKARM_OpenEx")?,
        close: sym(lib, "JLINKARM_Close")?,
        emu_get_num_devices: sym(lib, "JLINKARM_EMU_GetNumDevices")?,
        emu_get_list: sym(lib, "JLINKARM_EMU_GetList")?,
        emu_select_by_usbsn: sym(lib, "JLINKARM_EMU_SelectByUSBSN")?,
        emu_select_by_index: sym(lib, "JLINKARM_EMU_SelectByIndex")?,
        tif_select: sym(lib, "JLINKARM_TIF_Select")?,
        set_speed: sym(lib, "JLINKARM_SetSpeed")?,
        connect: sym(lib, "JLINKARM_Connect")?,
        is_connected: sym(lib, "JLINKARM_IsConnected")?,
        is_open: sym(lib, "JLINKARM_IsOpen")?,
        is_halted: sym(lib, "JLINKARM_IsHalted")?,
        go: sym(lib, "JLINKARM_Go")?,
        halt: sym(lib, "JLINKARM_Halt")?,
        reset: sym(lib, "JLINKARM_Reset")?,
        reset_no_halt: sym(lib, "JLINKARM_ResetNoHalt")?,
        exec_command: sym(lib, "JLINKARM_ExecCommand")?,
        get_dll_version: sym(lib, "JLINKARM_GetDLLVersion")?,
        get_sn: sym(lib, "JLINKARM_GetSN")?,
        device_get_index: sym(lib, "JLINKARM_DEVICE_GetIndex")?,
        device_get_info: sym(lib, "JLINKARM_DEVICE_GetInfo")?,
        core_to_name: sym(lib, "JLINKARM_Core2CoreName")?,
        rtt_control: sym(lib, "JLINK_RTTERMINAL_Control")?,
        rtt_read: sym(lib, "JLINK_RTTERMINAL_Read")?,
    })
}

fn c_char_array_to_string(a: &[c_char]) -> String {
    let bytes: Vec<u8> = a.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn c_char_slice_to_string(a: &[c_char]) -> String {
    c_char_array_to_string(a)
}

fn friendly_connect_error(code: i32) -> String {
    let hint = match code {
        -256 => "与 J-Link 无连接（调试器可能已被其他程序占用，如 RTT Viewer / GDB Server）",
        -257 => "J-Link 通信错误",
        -259 => "目标板未供电（VTref < 1V）",
        -261 => "未找到目标 CPU，请检查接线与芯片型号",
        -264 => "接口状态错误（如 TCK 电平异常），请检查 SWD/JTAG 接线",
        -273 => "未选择目标芯片，请先在连接面板选择芯片型号",
        _ => "连接目标失败",
    };
    format!("{}（错误码 {}）", hint, code)
}
