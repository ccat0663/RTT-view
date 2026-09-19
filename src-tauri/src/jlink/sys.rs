//! Raw FFI declarations for the SEGGER J-Link host DLL (`JLink_x64.dll`).
//!
//! Constants and struct layouts were verified against the J-Link SDK headers
//! (V6.94a / V6.98a) and cross-checked with the `pylink` bindings. They are the
//! authoritative specification; do not "tidy" field names or reorder fields.
//!
//! Calling convention: the SDK marks functions `STDCALL`, which on 64-bit
//! Windows is identical to the single Microsoft x64 convention, so `extern "C"`
//! is correct here. This code targets a 64-bit process and `JLink_x64.dll`.

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::os::raw::{c_char, c_int, c_uint, c_void};

pub type U8 = u8;
pub type U32 = u32;
pub type U64 = u64;

// ---------------------------------------------------------------------------
// Host interfaces accepted by JLINKARM_EMU_GetList() (bit flags)
// ---------------------------------------------------------------------------
pub const JLINKARM_HOSTIF_USB: c_int = 1 << 0;
pub const JLINKARM_HOSTIF_IP: c_int = 1 << 1;

// ---------------------------------------------------------------------------
// Target interfaces (JLINKARM_TIF_Select)
// ---------------------------------------------------------------------------
pub const JLINKARM_TIF_JTAG: c_int = 0;
pub const JLINKARM_TIF_SWD: c_int = 1;
pub const JLINKARM_TIF_FINE: c_int = 3;
pub const JLINKARM_TIF_ICSP: c_int = 4;
pub const JLINKARM_TIF_SPI: c_int = 5;
pub const JLINKARM_TIF_C2: c_int = 6;

// ---------------------------------------------------------------------------
// RTT terminal commands (JLINK_RTTERMINAL_Control)
// ---------------------------------------------------------------------------
pub const RTTERMINAL_CMD_START: U32 = 0;
pub const RTTERMINAL_CMD_STOP: U32 = 1;
pub const RTTERMINAL_CMD_GETDESC: U32 = 2;
pub const RTTERMINAL_CMD_GETNUMBUF: U32 = 3;
pub const RTTERMINAL_CMD_GETSTAT: U32 = 4;

// RTT buffer directions.
pub const RTTERMINAL_DIR_UP: U32 = 0;
pub const RTTERMINAL_DIR_DOWN: U32 = 1;

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// `JLINK_RTTERMINAL_START` — pass a zeroed value to auto-search the control block.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RttTerminalStart {
    pub ConfigBlockAddress: U32,
    pub Dummy0: U32,
    pub Dummy1: U32,
    pub Dummy2: U32,
}

/// `JLINK_RTTERMINAL_STOP`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RttTerminalStop {
    pub InvalidateTargetCB: U8,
    pub acDummy: [U8; 3],
    pub Dummy0: U32,
    pub Dummy1: U32,
    pub Dummy2: U32,
}

/// `JLINK_RTTERMINAL_BUFDESC` — set `BufferIndex` on input, other fields on output.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RttBufDesc {
    pub BufferIndex: c_int,
    pub Direction: U32,
    pub acName: [c_char; 32],
    pub SizeOfBuffer: U32,
    pub Flags: U32,
}

/// `JLINK_RTTERMINAL_STATUS`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RttTerminalStatus {
    pub NumBytesTransferred: U32,
    pub NumBytesRead: U32,
    pub HostOverflowCount: c_int,
    pub IsRunning: c_int,
    pub NumUpBuffers: c_int,
    pub NumDownBuffers: c_int,
    pub OverflowMask: U32, // was Dummy/Reserved before V6.93a
    pub Dummy1: U32,
}

/// `JLINKARM_EMU_CONNECT_INFO` — exactly 264 bytes on 64-bit.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EmuConnectInfo {
    pub SerialNumber: U32,
    pub Connection: c_uint,
    pub USBAddr: U32,
    pub aIPAddr: [U8; 16],
    pub Time: c_int,
    pub Time_us: U64,
    pub HWVersion: U32,
    pub abMACAddr: [U8; 6],
    pub acProduct: [c_char; 32],
    pub acNickName: [c_char; 32],
    pub acFWString: [c_char; 112],
    pub IsDHCPAssignedIP: c_char,
    pub IsDHCPAssignedIPIsValid: c_char,
    pub NumIPConnections: c_char,
    pub NumIPConnectionsIsValid: c_char,
    pub aPadding: [U8; 34],
}

impl Default for EmuConnectInfo {
    fn default() -> Self {
        Self {
            SerialNumber: 0,
            Connection: 0,
            USBAddr: 0,
            aIPAddr: [0; 16],
            Time: 0,
            Time_us: 0,
            HWVersion: 0,
            abMACAddr: [0; 6],
            acProduct: [0; 32],
            acNickName: [0; 32],
            acFWString: [0; 112],
            IsDHCPAssignedIP: 0,
            IsDHCPAssignedIPIsValid: 0,
            NumIPConnections: 0,
            NumIPConnectionsIsValid: 0,
            aPadding: [0; 34],
        }
    }
}

// ---------------------------------------------------------------------------
// Function table
// ---------------------------------------------------------------------------

/// Resolved function pointers. Built once per loaded DLL, then shared (all calls
/// are serialized by the caller via a mutex, because the DLL is a process-wide
/// singleton and is not re-entrant).
#[derive(Clone, Copy)]
pub struct RawApi {
    /// `const char* JLINKARM_Open(void)` — NULL on success, else error string.
    pub open: unsafe extern "C" fn() -> *const c_char,
    /// `void JLINKARM_OpenEx(JLINKARM_LOG*, JLINKARM_LOG*)`
    pub open_ex: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_char,
    pub close: unsafe extern "C" fn(),
    pub emu_get_num_devices: unsafe extern "C" fn() -> U32,
    pub emu_get_list: unsafe extern "C" fn(c_int, *mut EmuConnectInfo, c_int) -> c_int,
    pub emu_select_by_usbsn: unsafe extern "C" fn(U32) -> c_int,
    pub emu_select_by_index: unsafe extern "C" fn(U32) -> U32,
    pub tif_select: unsafe extern "C" fn(c_int) -> c_int,
    pub set_speed: unsafe extern "C" fn(U32),
    pub connect: unsafe extern "C" fn() -> c_int,
    pub is_connected: unsafe extern "C" fn() -> c_char,
    /// `char JLINKARM_IsOpen(void)` — non-zero when the DLL currently holds an
    /// open session. `JLINKARM_EMU_SelectByUSBSN` fails while this is set.
    pub is_open: unsafe extern "C" fn() -> c_char,
    /// `int JLINKARM_IsHalted(void)` — non-zero when the CPU is stopped.
    pub is_halted: unsafe extern "C" fn() -> c_int,
    /// `void JLINKARM_Go(void)` — resume the CPU (`JLINKARM_GoAllowSim` variant
    /// also exists; plain Go is what we want).
    pub go: unsafe extern "C" fn(),
    /// `void JLINKARM_Halt(void)` — stop the CPU.
    pub halt: unsafe extern "C" fn(),
    /// `int JLINKARM_Reset(void)` — reset the target; the CPU is left halted.
    pub reset: unsafe extern "C" fn() -> c_int,
    /// `void JLINKARM_ResetNoHalt(void)` — reset the target and leave the CPU
    /// running. Preferred over `JLINKARM_Reset`, which halts the core and would
    /// stop the firmware from printing.
    pub reset_no_halt: unsafe extern "C" fn(),
    pub exec_command: unsafe extern "C" fn(*const c_char, *mut c_char, c_int) -> c_int,
    pub get_dll_version: unsafe extern "C" fn() -> U32,
    /// `U32 JLINKARM_GetSN(void)` — serial number of the currently opened probe.
    /// Valid only after `JLINKARM_Open`; used to report which probe is actually in
    /// use when the user let the app pick automatically.
    pub get_sn: unsafe extern "C" fn() -> U32,
    pub device_get_index: unsafe extern "C" fn(*const c_char) -> c_int,
    /// `int JLINKARM_DEVICE_GetInfo(int DeviceIndex, JLINKARM_DEVICE_INFO* pInfo)`
    /// Returns < 0 once the index runs past the built-in device database, which
    /// is how the database is enumerated (there is no "count" function).
    pub device_get_info: unsafe extern "C" fn(c_int, *mut JlinkDeviceInfo) -> c_int,
    /// `void JLINKARM_Core2CoreName(U32 Core, char* pBuffer, unsigned BufferSize)`
    pub core_to_name: unsafe extern "C" fn(U32, *mut c_char, c_uint),
    pub rtt_control: unsafe extern "C" fn(U32, *mut c_void) -> c_int,
    pub rtt_read: unsafe extern "C" fn(U32, *mut c_char, U32) -> c_int,
}

// ---------------------------------------------------------------------------
// Device database (JLINKARM_DEVICE_INFO and its nested types)
//
// The J-Link DLL ships a *built-in* device database; `JLinkDevices.xml` only
// adds/overrides entries, which is why that file lists a few hundred devices
// while the DLL knows thousands. These types come from the J-Link SDK header
// (V6.98a) and must match byte-for-byte: `Core` sits after several large arrays,
// so a single wrong field size would misalign it.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlashAreaInfo {
    pub Addr: U32,
    pub Size: U32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RamAreaInfo {
    pub Addr: U32,
    pub Size: U32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlashBankInfo {
    pub sBankName: *const c_char,
    pub sAlgoFile: *const c_char,
    pub AlgoType: U32,
    pub BaseAddr: U32,
    pub paBlockInfo: *const c_void,
    pub pAlgoInfo: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PCodeInfo {
    pub pPCode: *const c_void,
    pub NumBytes: U32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlashBankInfoExt {
    pub pRAMCodeTurbo_LE: *const c_void,
    pub pRAMCodeTurbo_BE: *const c_void,
    pub pRAMCode_LE: *const c_void,
    pub pRAMCode_BE: *const c_void,
    pub SizeRAMCodeTurbo_LE: U32,
    pub SizeRAMCodeTurbo_BE: U32,
    pub SizeRAMCode_LE: U32,
    pub SizeRAMCode_BE: U32,
    pub pPCode: *const c_void,
    pub SizePCode: U32,
}

pub const JLINKARM_DEVICE_MAX_NUM_FLASH_BANKS: usize = 16;

/// `JLINKARM_DEVICE_INFO` — 2400 bytes on 64-bit (verified against the DLL).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct JlinkDeviceInfo {
    pub SizeOfStruct: U32,
    pub sName: *const c_char,
    pub CoreId: U32,
    pub FlashAddr: U32,
    pub RAMAddr: U32,
    pub EndianMode: c_char,
    pub FlashSize: U32,
    pub RAMSize: U32,
    pub sManu: *const c_char,
    pub aFlashArea: [FlashAreaInfo; 32],
    pub aRAMArea: [RamAreaInfo; 32],
    pub Core: U32,
    pub aFlashBank: [FlashBankInfo; JLINKARM_DEVICE_MAX_NUM_FLASH_BANKS],
    pub aPCodes: [PCodeInfo; 10],
    pub aFlashBankExt: [FlashBankInfoExt; JLINKARM_DEVICE_MAX_NUM_FLASH_BANKS],
    pub sNote: *const c_char,
}

impl JlinkDeviceInfo {
    /// All-zero value is a valid empty template; callers must then set
    /// `SizeOfStruct`, which the DLL uses to validate the ABI.
    pub fn zeroed() -> Self {
        // Safe: the type is plain-old-data with no invalid bit patterns.
        unsafe { std::mem::zeroed() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn struct_layout_matches_sdk() {
        assert_eq!(size_of::<RttTerminalStart>(), 16);
        assert_eq!(size_of::<RttTerminalStop>(), 16);
        assert_eq!(size_of::<RttBufDesc>(), 48);
        assert_eq!(size_of::<RttTerminalStatus>(), 32);
        assert_eq!(size_of::<EmuConnectInfo>(), 264);
    }

    #[test]
    fn device_info_layout_matches_dll() {
        // The DLL validates `SizeOfStruct`, and `Core` sits after the arrays, so
        // the total size is the one fact that pins the whole layout. 2400 is
        // what the installed J-Link DLL expects on 64-bit (verified by calling
        // JLINKARM_DEVICE_GetInfo and enumerating the entire database).
        assert_eq!(size_of::<JlinkDeviceInfo>(), 2400);
        assert_eq!(size_of::<FlashAreaInfo>(), 8);
        assert_eq!(size_of::<RamAreaInfo>(), 8);
        assert_eq!(size_of::<FlashBankInfo>(), 40);
        assert_eq!(size_of::<PCodeInfo>(), 16);
        assert_eq!(size_of::<FlashBankInfoExt>(), 64);

        // Offsets the reader depends on.
        use std::mem::offset_of;
        assert_eq!(offset_of!(JlinkDeviceInfo, sName), 8);
        assert_eq!(offset_of!(JlinkDeviceInfo, FlashSize), 32);
        assert_eq!(offset_of!(JlinkDeviceInfo, RAMSize), 36);
        assert_eq!(offset_of!(JlinkDeviceInfo, sManu), 40);
        assert_eq!(offset_of!(JlinkDeviceInfo, Core), 560);
        assert_eq!(offset_of!(JlinkDeviceInfo, sNote), 2392);
    }
}
