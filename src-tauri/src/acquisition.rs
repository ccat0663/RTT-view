//! The single thread that owns the J-Link DLL.
//!
//! The J-Link DLL is a process-wide singleton and is not re-entrant, so *every*
//! DLL call in this application happens on this thread. The GUI talks to it via
//! a command channel and receives batches of log lines over a Tauri IPC channel.

use crate::config::Settings;
use crate::discovery;
use crate::jlink::{self, JLinkDll, ProbeInfo, RttBufferInfo};
use crate::line_assembler::{decode_line, LineAssembler};
use crate::logging::{FileSink, SaveFormat};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;

/// One displayed/logged line.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Unix epoch milliseconds (wall clock, host receive time).
    pub ts_ms: i64,
    /// Seconds since the session started (monotonic, immune to clock changes).
    pub rel_s: f64,
    /// Which RTT up buffer this came from, e.g. `ch0` / `ch1 (Terminal)`.
    /// Present only when capturing all channels, where lines would otherwise be
    /// indistinguishable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ch: Option<String>,
    pub text: String,
}

/// One RTT up buffer being captured: its index, display label, and its own line
/// reassembler.
struct Capture {
    index: u32,
    label: String,
    assembler: LineAssembler,
}

/// Events pushed to the frontend.
///
/// Two serde attributes are needed here: `rename_all` renames the variants
/// (`State` → `state`) and `rename_all_fields` renames the fields *inside*
/// struct variants (`bytes_per_sec` → `bytesPerSec`). Without the latter the
/// frontend sees snake_case keys and every numeric stat renders as
/// `undefined`/`NaN`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AcqEvent {
    /// Connection state or capability updates.
    State {
        state: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        dll_version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        probes: Option<Vec<ProbeInfo>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        buffers: Option<Vec<RttBufferInfo>>,
    },
    Lines {
        lines: Vec<LogLine>,
    },
    /// Full device list read from the DLL's built-in database.
    Devices {
        devices: Vec<crate::jlink::DeviceEntry>,
    },
    Stats {
        bytes_per_sec: f64,
        lines_per_sec: f64,
        host_overflow: i64,
        total_lines: u64,
        saved_bytes: u64,
        saving: bool,
    },
    Error {
        message: String,
    },
}

/// Commands sent from the GUI to the acquisition thread.
pub enum AcqCommand {
    RefreshDevices { dll_path: String },
    RefreshProbes { dll_path: String },
    ValidateDevice { name: String },
    Connect(Box<ConnectConfig>),
    Test(Box<ConnectConfig>),
    Disconnect,
    /// Reset the target and leave it running.
    Reset,
    /// Clear the displayed log and reset the line counter.
    ClearLog,
    StartSave { path: PathBuf, format: SaveFormat, append: bool },
    StopSave,
    SetDisplay(bool),
    #[allow(dead_code)]
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct ConnectConfig {
    pub dll_path: String,
    pub probe_serial: u32,
    pub device: String,
    pub interface: String,
    pub speed_khz: u32,
    pub up_channel: u32,
    /// Capture every RTT up buffer instead of only `up_channel`.
    pub capture_all: bool,
    /// Reset the target (and leave it running) as part of connecting.
    pub reset_on_connect: bool,
}

impl ConnectConfig {
    #[allow(dead_code)]
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            dll_path: s.dll_path.clone(),
            probe_serial: s.probe_serial,
            device: s.device.clone(),
            interface: s.interface.clone(),
            speed_khz: s.speed_khz,
            up_channel: s.up_channel,
            capture_all: s.capture_all,
            reset_on_connect: s.reset_on_connect,
        }
    }
}

/// Handle used by Tauri commands to talk to the acquisition thread.
///
/// Cloning shares the sender, the shutdown flag and the join handle, so every
/// clone refers to the same thread. Joining is idempotent (the handle is taken on
/// the first call, so a later one returns immediately), but only the shutdown path
/// should call [`AcqHandle::shutdown`].
#[derive(Clone)]
pub struct AcqHandle {
    tx: Sender<AcqCommand>,
    /// Set to ask the thread to stop, including from inside a blocking wait.
    shutdown: Arc<AtomicBool>,
    /// `Arc<Mutex<..>>` so the handle can be taken (and joined) exactly once.
    thread: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl AcqHandle {
    pub fn send(&self, cmd: AcqCommand) -> Result<(), String> {
        self.tx.send(cmd).map_err(|e| format!("采集线程已退出: {}", e))
    }

    /// Stop the acquisition thread, waiting up to `timeout` for it to finish.
    ///
    /// Must run before the process exits: the thread owns the J-Link DLL, and
    /// terminating while it is executing inside the DLL can deadlock shutdown and
    /// leave the app running in the background.
    ///
    /// The wait is bounded because some J-Link calls block for a long time inside
    /// the DLL (measured: `JLINKARM_ExecCommand` with an unknown device name took
    /// over ten seconds) and the DLL exposes no cancellation API, so the thread
    /// cannot be interrupted. In the common case the thread stops in milliseconds;
    /// if it is stuck in such a call we give up rather than hang the UI, and let
    /// process teardown reclaim it.
    ///
    /// Returns whether the thread finished within the timeout.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.shutdown.store(true, Ordering::Relaxed);
        // Wake a thread sleeping in the command loop.
        let _ = self.tx.send(AcqCommand::Shutdown);

        let handle = self.thread.lock().ok().and_then(|mut guard| guard.take());
        let Some(handle) = handle else {
            return true; // already joined (or never started)
        };

        // Poll `is_finished` instead of blocking in `join`, so a stuck DLL call
        // cannot hold the exit path indefinitely.
        let deadline = Instant::now() + timeout;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
            true
        } else {
            eprintln!(
                "[acquire] 采集线程在 {:?} 内未结束（可能正阻塞于 J-Link DLL 调用）；交由进程退出回收",
                timeout
            );
            false
        }
    }
}

/// Spawn the acquisition thread and return a handle.
pub fn spawn(channel: Channel<AcqEvent>) -> AcqHandle {
    let (tx, rx) = crossbeam_channel::unbounded();
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_flag = Arc::clone(&shutdown);
    let handle = std::thread::Builder::new()
        .name("jlink-acq".into())
        .spawn(move || {
            let mut acq = Acquisition::new(channel, thread_flag);
            acq.run(rx);
        })
        .expect("无法创建采集线程");
    AcqHandle {
        tx,
        shutdown,
        thread: Arc::new(std::sync::Mutex::new(Some(handle))),
    }
}

/// Timeout used when polling for commands; also the read-loop tick.
const TICK: Duration = Duration::from_millis(2);
/// Flush accumulated lines to the UI at most this often.
const FLUSH_INTERVAL: Duration = Duration::from_millis(30);
/// Emit throughput stats this often.
const STATS_INTERVAL: Duration = Duration::from_millis(1000);
/// Lines accumulated before an early flush.
const FLUSH_LINES: usize = 1024;
/// Idle time after which an incomplete line is flushed.
const PARTIAL_TIMEOUT: Duration = Duration::from_millis(200);
/// Bound on a single RTT read.
const READ_CHUNK: usize = 8192;

struct Acquisition {
    channel: Channel<AcqEvent>,
    /// Set by the UI thread on exit; the loop checks it so the thread can stop
    /// promptly (including while waiting for the RTT control block).
    shutdown: Arc<AtomicBool>,

    dll: Option<JLinkDll>,
    dll_path_loaded: Option<PathBuf>,
    session_open: bool,
    connected: bool,
    /// Set once `JLINKARM_Connect` succeeds, i.e. the probe reached the chip.
    ///
    /// Distinguishes "could not attach to the target" from "attached fine but the
    /// RTT control block was not found" — different problems with different fixes,
    /// and previously both were reported as a bare "连接失败".
    chip_connected: bool,

    up_channel: u32,
    /// When set, poll every RTT up buffer instead of just `up_channel`.
    capture_all: bool,

    /// One reassembler per captured up buffer. Each channel is an independent
    /// byte stream, so sharing one buffer across channels would splice lines
    /// together.
    captures: Vec<Capture>,
    /// When the most recent RTT byte arrived (drives partial-line flushing).
    last_data_at: Instant,
    t0: Option<Instant>,
    pending: Vec<LogLine>,

    // Save sink.
    sink: Option<FileSink>,
    display_enabled: bool,

    // Stats accounting.
    last_flush: Instant,
    last_stats: Instant,
    bytes_since_stats: u64,
    lines_since_stats: u64,
    total_lines: u64,
    host_overflow: i64,
}

impl Acquisition {
    fn new(channel: Channel<AcqEvent>, shutdown: Arc<AtomicBool>) -> Self {
        let now = Instant::now();
        Self {
            channel,
            shutdown,
            dll: None,
            dll_path_loaded: None,
            session_open: false,
            connected: false,
            chip_connected: false,
            up_channel: 0,
            capture_all: false,
            captures: Vec::new(),
            last_data_at: now,
            t0: None,
            pending: Vec::new(),
            sink: None,
            display_enabled: true,
            last_flush: now,
            last_stats: now,
            bytes_since_stats: 0,
            lines_since_stats: 0,
            total_lines: 0,
            host_overflow: 0,
        }
    }

    fn emit(&self, ev: AcqEvent) {
        let _ = self.channel.send(ev);
    }

    fn state(&self, state: &str, message: impl Into<String>) {
        let message = message.into();
        // Also log to stderr: the UI shows the message, but during development
        // (and when the UI cannot render) the terminal is the only place to see
        // where a connection attempt got to.
        eprintln!("[acquire] state={state}: {message}");
        self.emit(AcqEvent::State {
            state: state.to_string(),
            message,
            dll_version: self.dll.as_ref().map(|d| d.version_string()),
            probes: None,
            buffers: None,
        });
    }

    fn error(&self, message: impl Into<String>) {
        let message = message.into();
        eprintln!("[acquire] ERROR: {message}");
        self.emit(AcqEvent::Error { message });
    }

    /// Whether the DLL holds a live session that RTT commands may be issued to.
    ///
    /// See [`rtt_query_allowed`] for why this gate exists.
    fn rtt_is_live(&self) -> bool {
        rtt_query_allowed(self.connected, self.session_open)
    }

    fn run(&mut self, rx: Receiver<AcqCommand>) {
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                // Close the probe / log file before the thread ends, so the DLL is
                // not in use when the process tears it down.
                self.teardown();
                self.stop_save();
                return;
            }

            // Drain commands.
            while let Ok(cmd) = rx.try_recv() {
                if !self.handle(cmd) {
                    return; // Shutdown
                }
            }

            if self.connected {
                self.pump_rtt();
            }

            let now = Instant::now();
            // A partial line (prompt without a newline) is flushed once the
            // stream has gone idle, matching how serial terminals behave.
            if self.captures.iter().any(|c| !c.assembler.is_empty())
                && now.duration_since(self.last_data_at) >= PARTIAL_TIMEOUT
            {
                self.flush_partial();
            }
            if !self.pending.is_empty()
                && (now.duration_since(self.last_flush) >= FLUSH_INTERVAL
                    || self.pending.len() >= FLUSH_LINES)
            {
                self.flush_lines();
            }
            if now.duration_since(self.last_stats) >= STATS_INTERVAL {
                self.emit_stats(now);
            }

            std::thread::sleep(if self.connected { TICK } else { TICK * 5 });
        }
    }

    /// Returns false to stop the thread.
    fn handle(&mut self, cmd: AcqCommand) -> bool {
        match cmd {
            AcqCommand::Shutdown => {
                // Close the log file explicitly so buffered lines are flushed;
                // teardown() no longer does this, since saving outlives a session.
                self.stop_save();
                self.teardown();
                return false;
            }
            AcqCommand::RefreshDevices { dll_path } => self.refresh_devices(&dll_path),
            AcqCommand::RefreshProbes { dll_path } => self.refresh_probes(&dll_path),
            AcqCommand::ValidateDevice { name } => self.validate_device(&name),
            AcqCommand::Connect(cfg) => self.connect(&cfg),
            AcqCommand::Test(cfg) => self.test_connection(&cfg),
            AcqCommand::Disconnect => {
                self.teardown();
                self.state("disconnected", "已断开");
            }
            AcqCommand::Reset => self.reset_target(),
            AcqCommand::ClearLog => {
                // The counter is the app's own statistic, so clearing the view must
                // zero it too — otherwise the status bar kept counting lines that
                // are no longer displayed.
                self.total_lines = 0;
                self.lines_since_stats = 0;
                self.pending.clear();
            }
            AcqCommand::StartSave { path, format, append } => self.start_save(&path, format, append),
            AcqCommand::StopSave => self.stop_save(),
            AcqCommand::SetDisplay(on) => self.display_enabled = on,
        }
        true
    }

    // -- DLL / discovery ---------------------------------------------------

    /// Ensure the DLL is loaded, optionally (re)loading if the path changed.
    fn ensure_dll(&mut self, dll_path: &str) -> Result<(), String> {
        let wanted = discovery::find_jlink_dll(if dll_path.is_empty() { None } else { Some(dll_path) })
            .map_err(|e| format!("{:#}", e))?;

        if let Some(loaded) = &self.dll_path_loaded {
            if *loaded == wanted {
                return Ok(());
            }
            if self.session_open {
                return Err(
                    "J-Link 会话已打开，DLL 加载后无法更换。请断开连接并重启程序后再切换 DLL。"
                        .to_string(),
                );
            }
        }

        let reloading = self.dll.is_some();
        eprintln!(
            "[acquire] ensure_dll: {} -> {}",
            if reloading { "RELOADING (dropping previous instance!)" } else { "first load" },
            wanted.display()
        );
        let dll = JLinkDll::load(&wanted).map_err(|e| format!("{:#}", e))?;
        eprintln!("[acquire] ensure_dll: loaded, IsOpen={}", dll.is_open());
        self.dll = Some(dll);
        self.dll_path_loaded = Some(wanted);
        Ok(())
    }

    fn refresh_devices(&mut self, dll_path: &str) {
        if let Err(e) = self.ensure_dll(dll_path) {
            self.error(e);
            return;
        }
        if let Some(dll) = &self.dll {
            dll.suppress_gui();
        }
        let Some(dll) = &self.dll else { return };
        // The device list lives in the DLL's built-in database; `JLinkDevices.xml`
        // only adds/overrides a few hundred entries, so it must not be used as
        // the catalogue (it would hide thousands of supported parts).
        let devices = dll.list_devices();
        if devices.is_empty() {
            self.error("未能从 J-Link DLL 读取设备库（可能是 DLL 版本过旧或结构体不匹配）");
            return;
        }
        let _ = self.channel.send(AcqEvent::State {
            state: "devices".into(),
            message: format!("已加载 {} 个设备", devices.len()),
            dll_version: Some(dll.version_string()),
            probes: None,
            buffers: None,
        });
        eprintln!("[acquire] refresh_devices done: IsOpen={}", dll.is_open());
        let _ = self.channel.send(AcqEvent::Devices { devices });
    }

    fn refresh_probes(&mut self, dll_path: &str) {
        if let Err(e) = self.ensure_dll(dll_path) {
            self.error(e);
            return;
        }
        if let Some(dll) = &self.dll {
            dll.suppress_gui();
        }
        let Some(dll) = &self.dll else { return };
        match dll.list_probes() {
            Ok(probes) => {
                let msg = if probes.is_empty() {
                    "未检测到 J-Link 调试器".to_string()
                } else {
                    format!("检测到 {} 个调试器", probes.len())
                };
                eprintln!("[acquire] refresh_probes done: IsOpen={}", dll.is_open());
                let _ = self.channel.send(AcqEvent::State {
                    state: "probes".into(),
                    message: msg,
                    dll_version: Some(dll.version_string()),
                    probes: Some(probes),
                    buffers: None,
                });
            }
            Err(e) => self.error(format!("{:#}", e)),
        }
    }

    fn validate_device(&mut self, name: &str) {
        let Some(dll) = &self.dll else {
            self.error("尚未加载 J-Link DLL");
            return;
        };
        match dll.device_index(name) {
            Ok(idx) if idx >= 0 => self.state("deviceOk", format!("芯片 '{}' 有效", name)),
            Ok(idx) => self.error(format!("DLL 不识别芯片 '{}'（索引 {}）", name, idx)),
            Err(e) => self.error(format!("{:#}", e)),
        }
    }

    // -- connect ------------------------------------------------------------

    fn open_session(&mut self, cfg: &ConnectConfig) -> Result<(), String> {
        let t0 = Instant::now();
        // Includes the GUI-server count: with no probe attached, some J-Link calls
        // make the DLL spawn it (and its emulator-selection dialog). Logging it per
        // step shows exactly which call is responsible.
        let step = |name: &str| eprintln!("[acquire] +{:>6.0?} {name}", t0.elapsed());

        self.ensure_dll(&cfg.dll_path)?;
        step("dll ready");

        // Re-assert GUI suppression before any call that can raise a J-Link
        // dialog. It was set once at DLL load, but the DLL's device-change
        // handling can drop it — and with no probe attached, enumeration and
        // Open are exactly the calls that make J-Link pop its "Emulator
        // selection" window. Re-sending is cheap and idempotent.
        if let Some(dll) = &self.dll {
            dll.suppress_gui();
        }

        // A previous session may still be open — pressing 连接 twice, or a connect
        // that raced the auto-connect at startup. `JLINKARM_EMU_SelectByUSBSN`
        // returns -1 while the DLL is open, so the probe selection below would
        // fail. Close any stale session first, without disturbing file logging.
        //
        // `stop_rtt = true`: this is a *live* session by construction — we are
        // about to connect, and connect refuses to run without an attached probe —
        // so stopping RTT is safe and needed to silence the old capture.
        self.reset_session(true);
        if let Some(dll) = &self.dll {
            step(if dll.is_open() { "session still open (unexpected)" } else { "session clean" });
        }

        let dll = self.dll.as_ref().unwrap();

        // Gate on probe presence BEFORE calling anything that can open a session.
        //
        // This is not just a nicety. With no probe attached, `JLINKARM_Open` (and
        // selecting a probe) makes the J-Link software spawn `JLinkGUIServer.exe`
        // and pop its "Emulator selection" dialog — which steals focus and cannot
        // be dismissed from our UI. Clicking 连接 again only stacks another one,
        // which looks like "the dialog keeps coming back and will not close".
        // Enumerating attached probes is safe (it is the same call 「刷新」 uses),
        // so check first and refuse early rather than blunder into that path.
        let probes = dll
            .list_probes()
            .map_err(|e| format!("无法枚举调试器：{e:#}"))?;

        let visible = probes
            .iter()
            .map(|p| format!("SN {}", p.serial))
            .collect::<Vec<_>>()
            .join(", ");

        let attached: Vec<u32> = probes.iter().map(|p| p.serial).collect();
        match connect_gate(&attached, cfg.probe_serial) {
            ConnectGate::Ok => {}
            ConnectGate::NoProbes => {
                eprintln!("[acquire] connect refused: 未检测到调试器");
                return Err(
                    "未检测到 J-Link 调试器。请确认调试器已插好，点「刷新」确认能检测到后再连接。"
                        .to_string(),
                );
            }
            ConnectGate::SerialNotFound => {
                // The DLL would otherwise fail later with a bare -1, which says
                // nothing useful.
                eprintln!(
                    "[acquire] connect refused: 配置的 SN {} 不在 [{visible}] 中",
                    cfg.probe_serial
                );
                return Err(format!(
                    "配置的调试器（SN {}）当前未连接。已检测到：{visible}。请点「刷新」后重新选择调试器。",
                    cfg.probe_serial
                ));
            }
        }

        eprintln!(
            "[acquire] pre-connect: IsOpen={} IsConnected={} | DLL 可见调试器=[{}] | 配置序列号={}",
            dll.is_open(),
            dll.is_connected(),
            visible,
            cfg.probe_serial,
        );

        // Selection is a best-effort optimisation, not a requirement: with a single
        // probe `Open` picks it automatically. It is only attempted now that we
        // know a probe is present, so a failure cannot leave the DLL in the
        // no-device state that raises the dialog.
        if cfg.probe_serial != 0 {
            let mut attempt = 0;
            let mut selected = false;
            while attempt < 3 {
                attempt += 1;
                match dll.select_by_serial(cfg.probe_serial) {
                    Ok(()) => {
                        selected = true;
                        break;
                    }
                    Err(e) => {
                        eprintln!("[acquire] probe select attempt {attempt} failed: {e:#}");
                        std::thread::sleep(Duration::from_millis(150));
                    }
                }
            }
            if selected {
                step(if attempt > 1 {
                    "probe selected (after retry)"
                } else {
                    "probe selected"
                });
            } else {
                // Fall back to automatic selection. Only refuse if doing so would
                // be ambiguous, i.e. more than one probe is attached.
                let count = dll.list_probes().map(|p| p.len()).unwrap_or(1);
                if count > 1 {
                    return Err(format!(
                        "选择调试器序列号 {} 失败，且检测到 {count} 个调试器，无法自动选择。请在调试器下拉框中重新选择。",
                        cfg.probe_serial
                    ));
                }
                eprintln!(
                    "[acquire] probe select failed but only {count} probe(s) present — continuing with automatic selection"
                );
                step("probe select failed; using automatic selection");
            }
        } else {
            step("probe serial is 0 — using automatic selection");
        }
        match dll.open() {
            Ok(()) => {}
            Err(e) => {
                eprintln!(
                    "[acquire] open FAILED | IsOpen={} probes={:?}",
                    dll.is_open(),
                    dll.list_probes().map(|p| p.len()).unwrap_or(0)
                );
                return Err(format!("{:#}", e));
            }
        }
        self.session_open = true;
        step("dll opened");

        let tif = if cfg.interface.eq_ignore_ascii_case("JTAG") {
            jlink::JLINKARM_TIF_JTAG
        } else {
            jlink::JLINKARM_TIF_SWD
        };
        dll.set_interface(tif).map_err(|e| format!("{:#}", e))?;
        dll.set_speed(cfg.speed_khz);
        step("interface + speed set");

        if !cfg.device.is_empty() {
            dll.exec_command(&format!("device = {}", cfg.device))
                .map_err(|e| format!("{:#}", e))?;
            step("device selected");
        }

        dll.connect().map_err(|e| format!("{:#}", e))?;
        self.chip_connected = true;
        step("target connected");

        // "Reset and run", the equivalent of Keil's download option.
        //
        // `Connect` can leave the core halted (at the reset vector if the previous
        // tool left it there), in which case the firmware is not running and its
        // RTT control block never appears. Resetting and letting it run puts the
        // target into a known state before we look for RTT.
        if cfg.reset_on_connect {
            dll.rtt_stop();
            dll.reset_no_halt();
            step("target reset and running (reset_on_connect)");
        } else if dll.is_halted() {
            // Even without an explicit reset, a halted core produces no RTT output,
            // so resume it (SEGGER's own RTT Viewer does the same).
            dll.go();
            step("target resumed (was halted by Connect)");
        }

        {
            let flag = Arc::clone(&self.shutdown);
            dll.rtt_start_cancellable(Duration::from_secs(5), &|| {
                flag.load(Ordering::Relaxed)
            })
            .map_err(|e| format!("{:#}", e))?;
        }
        step("rtt started");

        self.up_channel = cfg.up_channel;
        self.capture_all = cfg.capture_all;
        self.connected = true;
        self.t0 = Some(Instant::now());
        self.captures = self.build_captures();
        Ok(())
    }

    /// Decide which up buffers to poll.
    ///
    /// In "all" mode every up buffer the target exposes is captured, each with
    /// its own reassembler; otherwise just the configured one. If the DLL cannot
    /// report the buffer count we fall back to the configured channel, so a
    /// connection never fails merely because enumeration did.
    fn build_captures(&self) -> Vec<Capture> {
        let Some(dll) = &self.dll else { return Vec::new() };

        if !self.capture_all {
            return vec![Capture {
                index: self.up_channel,
                label: format!("ch{}", self.up_channel),
                assembler: LineAssembler::new(),
            }];
        }

        let buffers = dll.rtt_up_buffers();
        if buffers.is_empty() {
            return vec![Capture {
                index: self.up_channel,
                label: format!("ch{}", self.up_channel),
                assembler: LineAssembler::new(),
            }];
        }
        buffers
            .into_iter()
            .map(|b| {
                let label = if b.name.is_empty() {
                    format!("ch{}", b.index)
                } else {
                    format!("ch{} ({})", b.index, b.name)
                };
                Capture { index: b.index as u32, label, assembler: LineAssembler::new() }
            })
            .collect()
    }

    fn connect(&mut self, cfg: &ConnectConfig) {
        // Always log the parameters a connect was asked for: without them a
        // failed attempt cannot be reproduced or reasoned about from the log.
        eprintln!(
            "[acquire] ===== connect requested =====\n\
             [acquire]   dll    : {}\n\
             [acquire]   probe  : serial={}\n\
             [acquire]   device : {}\n\
             [acquire]   iface  : {} @ {}kHz\n\
             [acquire]   channels: up={} capture_all={}",
            if cfg.dll_path.is_empty() { "<auto>" } else { &cfg.dll_path },
            cfg.probe_serial,
            if cfg.device.is_empty() { "<未指定>" } else { &cfg.device },
            cfg.interface,
            cfg.speed_khz,
            cfg.up_channel,
            cfg.capture_all
        );
        self.state("connecting", "正在连接…");
        match self.open_session(cfg) {
            Ok(()) => {
                let buffers = self.dll.as_ref().map(|d| d.rtt_up_buffers()).unwrap_or_default();
                let name = if cfg.device.is_empty() { "<未指定>".into() } else { cfg.device.clone() };
                let channels = if cfg.capture_all {
                    format!("，捕获 {} 个 Up 通道", buffers.len().max(1))
                } else {
                    format!("，Up 通道 {}", cfg.up_channel)
                };
                // Report the serial the DLL actually opened, not the requested one:
                // with automatic selection `cfg.probe_serial` is 0, which said
                // nothing useful.
                let actual_sn = self
                    .dll
                    .as_ref()
                    .map(|d| d.serial_number())
                    .filter(|sn| *sn != 0)
                    .unwrap_or(cfg.probe_serial);
                let _ = self.channel.send(AcqEvent::State {
                    state: "connected".into(),
                    message: format!(
                        "已连接：SN {actual_sn} / {} @ {}kHz{}",
                        name, cfg.speed_khz, channels
                    ),
                    dll_version: self.dll.as_ref().map(|d| d.version_string()),
                    probes: None,
                    buffers: Some(buffers),
                });
            }
            Err(e) => {
                // The numbered step trace above shows how far it got. Report the
                // two stages separately: reaching the chip but not finding the RTT
                // control block is a different problem (usually the firmware is not
                // running) from failing to attach at all, and it has a concrete
                // remedy — 「复位并连接」.
                let reached_chip = self.chip_connected;
                eprintln!(
                    "[acquire] connect FAILED (chip_connected={reached_chip}) — reason: {e}"
                );
                self.teardown();
                if reached_chip {
                    // Attached to the chip; only RTT is missing.
                    self.error("未找到 RTT 控制块（固件可能没有运行）");
                } else {
                    self.error(e);
                }
                self.state("disconnected", connect_failure_message(reached_chip));
            }
        }
    }

    /// Reset the target and keep it running.
    ///
    /// Resetting tears down the firmware's RTT control block, so the capture
    /// state from before is stale: the reassemblers are dropped (a half-received
    /// line must not be completed by post-reset bytes) and RTT is re-armed, since
    /// the control block reappears at a possibly different address once the
    /// firmware boots again.
    fn reset_target(&mut self) {
        let Some(dll) = &self.dll else {
            self.error("尚未连接，无法复位");
            return;
        };
        if !self.connected {
            self.error("尚未连接，无法复位");
            return;
        }

        eprintln!("[acquire] resetting target (ResetNoHalt)");
        self.state("resetting", "正在复位目标…");
        dll.rtt_stop();
        dll.reset_no_halt();

        // The CPU is running again, but the control block only exists once the
        // firmware has re-initialised RTT. Wait for it, tolerating a slow boot.
        let flag = Arc::clone(&self.shutdown);
        match dll.rtt_start_cancellable(Duration::from_secs(5), &|| flag.load(Ordering::Relaxed)) {
            Ok(()) => {
                // Drop stale partial lines and re-discover the channels: indices
                // are the same in practice, but the buffers are new.
                self.captures = self.build_captures();
                self.last_data_at = Instant::now();
                let n = self.captures.len();
                self.state("reset", format!("目标已复位并继续运行（重新捕获 {n} 个 Up 通道）"));
            }
            Err(e) => {
                // The reset itself worked; only RTT did not come back in time.
                self.error(format!("目标已复位，但 RTT 控制块未在超时内就绪：{e:#}"));
                self.state(
                    "connected",
                    "目标已复位；等待固件重新初始化 RTT（可点「断开」后重新连接）",
                );
            }
        }
    }

    fn test_connection(&mut self, cfg: &ConnectConfig) {
        self.state("testing", "正在测试连接…");
        match self.open_session(cfg) {
            Ok(()) => {
                self.state("testOk", "连接测试成功：目标可访问，RTT 控制块已找到");
                self.teardown();
            }
            Err(e) => {
                // Same two-stage distinction as a real connect: reaching the chip
                // but not finding RTT is a different result from failing to attach,
                // and it has a concrete remedy.
                let reached_chip = self.chip_connected;
                self.teardown();
                if reached_chip {
                    self.error("未找到 RTT 控制块（固件可能没有运行）");
                } else {
                    self.error(format!("连接测试失败：{e}"));
                }
                // Prefix with 测试 so it is clear which button produced this.
                self.state(
                    "disconnected",
                    format!("测试：{}", connect_failure_message(reached_chip)),
                );
            }
        }
    }

    /// Close any open probe session and forget the capture state.
    ///
    /// Idempotent and safe to call before a new connect. It does not touch the
    /// file sink, so an in-progress save keeps its handle.
    ///
    /// `stop_rtt` must be false when the link is already known to be gone (the
    /// probe was unplugged). Every `JLINK_RTTERMINAL_Control` call — STOP included
    /// — blocks and raises the emulator dialog on a session whose emulator has
    /// disappeared, so issuing STOP on the way out would hang the acquisition
    /// thread inside the DLL; see [`rtt_query_allowed`]. `Close` itself is safe
    /// there.
    fn reset_session(&mut self, stop_rtt: bool) {
        if let Some(dll) = &self.dll {
            if stop_rtt && self.connected {
                dll.rtt_stop();
            }
            // Trust the DLL, not our flag: a session can be open even if tracking
            // got out of step, and an open DLL makes the next SelectByUSBSN fail.
            if self.session_open || dll.is_open() {
                dll.close();
            }
        }
        self.connected = false;
        self.session_open = false;
        self.chip_connected = false;
        self.captures.clear();
        self.t0 = None;
    }

    /// Tear down the probe session, stopping RTT first.
    ///
    /// Deliberately does **not** stop file saving: saving is independent of the
    /// connection. With no data arriving, nothing is written; when the user
    /// reconnects, the same file continues to receive lines. Only the user's
    /// 「停止保存」 and process shutdown close the file.
    fn teardown(&mut self) {
        self.flush_lines();
        self.reset_session(true);
    }

    /// Tear down after the link was *lost* (probe unplugged, target powered off).
    ///
    /// Identical to [`Self::teardown`] except that no RTT command is issued: at
    /// this point the emulator is gone, and a STOP would block inside the DLL
    /// while it offers the network/IP dialog.
    fn teardown_after_loss(&mut self) {
        self.flush_lines();
        self.reset_session(false);
    }

    // -- data path ----------------------------------------------------------

    fn pump_rtt(&mut self) {
        if self.dll.is_none() {
            return;
        }
        let disconnected = {
            let dll = self.dll.as_ref().unwrap();
            !dll.is_connected()
        };
        if disconnected {
            // The probe is gone, so no RTT command may be issued on the way out.
            self.teardown_after_loss();
            self.error("与 J-Link 的连接已断开（调试器被拔出或目标掉电）");
            self.state("disconnected", "连接已断开");
            return;
        }

        // Poll each captured up buffer. There is no "read all channels" call in
        // the J-Link API, so this is one read per channel per tick.
        let mut buf = [0u8; READ_CHUNK];
        let mut total = 0usize;
        let capture_count = self.captures.len();
        for ci in 0..capture_count {
            let index = self.captures[ci].index;
            let mut collected: Vec<u8> = Vec::new();
            loop {
                let n = {
                    let dll = self.dll.as_ref().unwrap();
                    match dll.rtt_read(index, &mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    }
                };
                collected.extend_from_slice(&buf[..n]);
                if collected.len() >= READ_CHUNK * 4 {
                    break; // bound pathological growth
                }
            }
            if !collected.is_empty() {
                total += collected.len();
                self.push_bytes_for(ci, &collected);
            }
        }

        if total > 0 {
            self.bytes_since_stats += total as u64;
        }
    }

    /// Append raw bytes received on capture `ci` and emit every completed line.
    fn push_bytes_for(&mut self, ci: usize, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.last_data_at = Instant::now();
        let now = Instant::now();
        let label = self.captures[ci].label.clone();
        let lines = self.captures[ci].assembler.feed(data, now, SystemTime::now());
        if std::env::var_os("RTT_DEBUG").is_some() {
            eprintln!("[acquire] rx {} bytes on {}", data.len(), self.captures[ci].label);
        }
        for line in lines {
            self.emit_line(&line.raw, line.first_byte_at, line.first_byte_wall, Some(&label));
        }
    }

    /// Flush any trailing partial lines (e.g. a prompt without a newline).
    fn flush_partial(&mut self) {
        for ci in 0..self.captures.len() {
            if let Some(line) = self.captures[ci].assembler.take_partial() {
                let label = self.captures[ci].label.clone();
                self.emit_line(
                    &line.raw,
                    line.first_byte_at,
                    line.first_byte_wall,
                    Some(&label),
                );
            }
        }
    }

    fn emit_line(
        &mut self,
        raw: &[u8],
        instant: Instant,
        wall: SystemTime,
        channel: Option<&str>,
    ) {
        let text = decode_line(raw);
        let ts_ms = wall
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let rel_s = self
            .t0
            .map(|t0| instant.saturating_duration_since(t0).as_secs_f64())
            .unwrap_or(0.0);

        self.total_lines += 1;
        self.lines_since_stats += 1;

        // The channel tag is only carried when capturing all channels; a single
        // configured channel would just repeat the same prefix on every line.
        let ch = if self.capture_all {
            channel.map(|c| c.to_string())
        } else {
            None
        };

        // File sink always gets the line, regardless of display settings.
        if let Some(sink) = &mut self.sink {
            let wall_str = format_wall(ts_ms);
            let text_for_file = match &ch {
                Some(c) => format!("[{}] {}", c, text),
                None => text.clone(),
            };
            let _ = sink.write_line(&wall_str, &text_for_file);
        }

        if self.display_enabled {
            self.pending.push(LogLine { ts_ms, rel_s, ch, text });
        }
    }

    fn flush_lines(&mut self) {
        if let Some(sink) = &mut self.sink {
            let _ = sink.flush();
        }
        if !self.pending.is_empty() {
            let lines = std::mem::take(&mut self.pending);
            self.emit(AcqEvent::Lines { lines });
        }
        self.last_flush = Instant::now();
    }

    fn emit_stats(&mut self, now: Instant) {
        let secs = now.duration_since(self.last_stats).as_secs_f64().max(1e-6);
        // Only ask the DLL for the overflow counter while a session is live.
        //
        // `JLINK_RTTERMINAL_Control(GETSTAT)` is not a passive read: on a closed
        // DLL with no probe behind it, the DLL goes looking for an emulator, and
        // with nothing on USB it offers the network/IP dialog **and blocks in the
        // call until that dialog is dealt with**. Polling it unconditionally meant
        // that after a probe was unplugged the one-second stats tick kept raising
        // that dialog — and because each poll re-raised it, closing the dialog
        // only brought it back, which is the reported symptom. Skipping the call
        // when there is no session keeps the last known count, which is also the
        // only value that can be meaningful: with nothing connected, no new
        // overflow can occur.
        let host_overflow = if self.rtt_is_live() {
            self.dll
                .as_ref()
                .and_then(|d| d.rtt_status())
                .map(|s| s.HostOverflowCount as i64)
                .unwrap_or(self.host_overflow)
        } else {
            self.host_overflow
        };
        self.host_overflow = host_overflow;
        self.emit(AcqEvent::Stats {
            bytes_per_sec: self.bytes_since_stats as f64 / secs,
            lines_per_sec: self.lines_since_stats as f64 / secs,
            host_overflow,
            total_lines: self.total_lines,
            saved_bytes: self.sink.as_ref().map(|s| s.bytes_written()).unwrap_or(0),
            saving: self.sink.is_some(),
        });
        self.bytes_since_stats = 0;
        self.lines_since_stats = 0;
        self.last_stats = now;
    }

    fn start_save(&mut self, path: &std::path::Path, format: SaveFormat, append: bool) {
        self.stop_save();
        match FileSink::open(path, format, append) {
            Ok(sink) => {
                self.sink = Some(sink);
                self.state("saving", format!("正在保存到 {}", path.display()));
            }
            Err(e) => self.error(format!("{:#}", e)),
        }
    }

    fn stop_save(&mut self) {
        if let Some(mut sink) = self.sink.take() {
            let _ = sink.flush();
            self.state("saved", format!("已保存 {} 字节到 {}", sink.bytes_written(), sink.path.display()));
        }
    }
}

/// Message shown in the status bar when a connect attempt fails.
///
/// The two cases are genuinely different problems and need different fixes, so
/// they must not read the same:
/// * the probe reached the chip but the RTT control block was not found (usually
///   the firmware is not running — 「复位并连接」 fixes it);
/// * the probe could not attach to the chip at all.
fn connect_failure_message(chip_connected: bool) -> &'static str {
    if chip_connected {
        "芯片已连接，但未找到 RTT 控制块 — 请尝试「复位并连接」"
    } else {
        "连接失败"
    }
}

/// Why a connect attempt must be refused before touching the DLL session.
///
/// Split out as a pure function so the rule is testable: calling `Open` (or
/// selecting a probe) with no probe attached makes the J-Link software raise its
/// "Emulator selection" dialog, which cannot be dismissed from our UI.
#[derive(Debug, PartialEq, Eq)]
enum ConnectGate {
    Ok,
    NoProbes,
    SerialNotFound,
}

fn connect_gate(attached: &[u32], requested: u32) -> ConnectGate {
    if attached.is_empty() {
        return ConnectGate::NoProbes;
    }
    if requested != 0 && !attached.contains(&requested) {
        return ConnectGate::SerialNotFound;
    }
    ConnectGate::Ok
}

/// Whether RTT commands may be issued to the DLL right now.
///
/// These calls are not passive reads. Measured against J-Link V6.88a with no
/// probe attached: after `Open` fails and `Close` is called,
/// `JLINK_RTTERMINAL_Control(GETSTAT)` still spawns `JLinkGUIServer.exe`, offers
/// the network/IP dialog, and **does not return** until that dialog is dismissed.
/// So issuing one while no session is open does not merely waste a call — it
/// blocks the acquisition thread and raises a dialog that reappears on every
/// retry, with no probe on USB to satisfy it.
///
/// Both flags are required: `session_open` says the DLL has a Open→Connect
/// session, and `connected` says it is still the one we want (it is cleared when
/// a disconnect is noticed, before the next stats tick would fire).
fn rtt_query_allowed(connected: bool, session_open: bool) -> bool {
    connected && session_open
}

fn format_wall(ts_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ts_ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => ts_ms.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `shutdown()` must actually end the thread, not just signal it.
    ///
    /// This exercises the real handle mechanics (flag + command + join) without a
    /// J-Link probe: the thread body here only waits on the flag, mirroring how
    /// the acquisition loop behaves when idle. If the flag or the join were
    /// wrong, the join would block and the test would hang.
    #[test]
    fn shutdown_ends_the_thread_and_joins() {
        use std::sync::atomic::AtomicUsize;

        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_in = Arc::clone(&ticks);

        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                ticks_in.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let (tx, _rx) = crossbeam_channel::unbounded();
        let acq = AcqHandle {
            tx,
            shutdown,
            thread: Arc::new(std::sync::Mutex::new(Some(handle))),
        };

        // Let it run briefly so we know it was alive.
        std::thread::sleep(Duration::from_millis(20));
        assert!(ticks.load(Ordering::Relaxed) > 0, "thread never ran");

        let started = Instant::now();
        let finished = acq.shutdown(Duration::from_secs(2));
        assert!(finished, "an idle thread should stop well within the timeout");
        assert!(started.elapsed() < Duration::from_secs(2), "shutdown took too long");

        // A second shutdown must be a no-op, not a panic or a hang.
        assert!(acq.shutdown(Duration::from_secs(1)));
    }

    /// A thread stuck in a long call cannot be interrupted (the J-Link DLL has no
    /// cancellation API), so `shutdown` must give up after the timeout rather than
    /// block exit forever.
    #[test]
    fn shutdown_gives_up_when_the_thread_is_stuck() {
        let shutdown = Arc::new(AtomicBool::new(false));
        // Deliberately ignore the flag: simulates a thread blocked inside the DLL.
        let handle = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(30)));

        let (tx, _rx) = crossbeam_channel::unbounded();
        let acq = AcqHandle {
            tx,
            shutdown,
            thread: Arc::new(std::sync::Mutex::new(Some(handle))),
        };

        let started = Instant::now();
        let finished = acq.shutdown(Duration::from_millis(150));
        let elapsed = started.elapsed();
        assert!(!finished, "a stuck thread must report not-finished");
        assert!(
            elapsed >= Duration::from_millis(140) && elapsed < Duration::from_secs(2),
            "should wait about the timeout, took {elapsed:?}"
        );
    }

    #[test]
    fn connect_is_refused_before_touching_the_session() {
        // No probe attached: must refuse (calling Open here raises the J-Link
        // emulator-selection dialog).
        assert_eq!(connect_gate(&[], 0), ConnectGate::NoProbes);
        assert_eq!(connect_gate(&[], 69612427), ConnectGate::NoProbes);

        // A specific probe that is no longer attached: refuse with a clear reason
        // instead of letting the DLL fail later with an opaque -1.
        assert_eq!(connect_gate(&[111], 69612427), ConnectGate::SerialNotFound);
        assert_eq!(connect_gate(&[111, 222], 333), ConnectGate::SerialNotFound);

        // Present probe, and the automatic (0) or matching explicit choice: allow.
        assert_eq!(connect_gate(&[69612427], 0), ConnectGate::Ok);
        assert_eq!(connect_gate(&[69612427], 69612427), ConnectGate::Ok);
        assert_eq!(connect_gate(&[111, 69612427], 69612427), ConnectGate::Ok);
    }

    #[test]
    fn connect_failure_distinguishes_rtt_from_attach() {
        // Reaching the chip but not finding RTT must not read as a plain failure,
        // and it must point at the recovery button.
        let rtt = connect_failure_message(true);
        assert!(rtt.contains("芯片已连接"), "{rtt}");
        assert!(rtt.contains("RTT 控制块"), "{rtt}");
        assert!(rtt.contains("复位并连接"), "{rtt}");
        // A hard attach failure stays generic.
        assert_eq!(connect_failure_message(false), "连接失败");
        // The 测试 button reuses the same wording, prefixed for clarity.
        let tested = format!("测试：{}", connect_failure_message(true));
        assert!(tested.starts_with("测试："));
        assert!(tested.contains("复位并连接"), "{tested}");
    }

    /// RTT commands may only be issued while a live session exists.
    ///
    /// Regression: `emit_stats` called `JLINK_RTTERMINAL_Control(GETSTAT)` every
    /// second regardless of state. Measured on J-Link V6.88a with no probe, that
    /// call on a closed DLL spawns `JLinkGUIServer.exe`, raises the network/IP
    /// dialog, and does not return until the dialog is dismissed — so after an
    /// unplug the one-second tick re-raised it forever, which is the "keeps
    /// popping up and will not stay closed" symptom.
    #[test]
    fn rtt_commands_are_refused_without_a_live_session() {
        // Live: both flags set by a successful connect.
        assert!(rtt_query_allowed(true, true));

        // Unplugged: the disconnect check clears the flags before the next stats
        // tick, so the poll must not go out.
        assert!(!rtt_query_allowed(false, true));
        assert!(!rtt_query_allowed(true, false));
        assert!(!rtt_query_allowed(false, false));
    }

    /// The gate must be false in exactly the state teardown leaves behind, so a
    /// stats tick arriving just after an unplug cannot reach the DLL.
    #[test]
    fn the_state_after_an_unplug_blocks_rtt_queries() {
        // What `reset_session` leaves: both flags cleared. A tick in this state
        // must keep the last overflow count rather than call GETSTAT.
        assert!(
            !rtt_query_allowed(false, false),
            "a stats tick after teardown must not issue an RTT command"
        );
        // Only a successful connect re-opens the gate.
        assert!(rtt_query_allowed(true, true));
    }

    /// The frontend reads these events by their camelCase field names. A plain
    /// `rename_all` on the enum only renames the *variants*, leaving fields like
    /// `bytes_per_sec` unchanged — which silently produced `undefined`/`NaN` in
    /// the status bar. This pins the wire format so that cannot regress.
    #[test]
    fn stats_event_uses_camel_case_fields() {
        let ev = AcqEvent::Stats {
            bytes_per_sec: 1234.5,
            lines_per_sec: 10.0,
            host_overflow: 3,
            total_lines: 42,
            saved_bytes: 99,
            saving: true,
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "stats");
        assert_eq!(v["bytesPerSec"], 1234.5);
        assert_eq!(v["linesPerSec"], 10.0);
        assert_eq!(v["hostOverflow"], 3);
        assert_eq!(v["totalLines"], 42);
        assert_eq!(v["savedBytes"], 99);
        assert_eq!(v["saving"], true);
        // The snake_case spellings must NOT be present.
        assert!(v.get("bytes_per_sec").is_none(), "snake_case key leaked: {v}");
    }

    #[test]
    fn state_event_uses_camel_case_fields() {
        let ev = AcqEvent::State {
            state: "connected".into(),
            message: "ok".into(),
            dll_version: Some("V6.88a".into()),
            probes: None,
            buffers: None,
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "state");
        assert_eq!(v["dllVersion"], "V6.88a");
    }

    /// `LogLine` is a plain struct, so struct-level `rename_all` already works —
    /// this guards it and the channel tag.
    #[test]
    fn log_line_uses_camel_case_and_channel_tag() {
        let ev = AcqEvent::Lines {
            lines: vec![LogLine {
                ts_ms: 1,
                rel_s: 0.5,
                ch: Some("ch1".into()),
                text: "hi".into(),
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "lines");
        assert_eq!(v["lines"][0]["tsMs"], 1);
        assert_eq!(v["lines"][0]["relS"], 0.5);
        assert_eq!(v["lines"][0]["ch"], "ch1");
        assert_eq!(v["lines"][0]["text"], "hi");
    }
}
