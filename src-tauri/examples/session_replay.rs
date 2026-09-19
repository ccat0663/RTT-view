//! Replay realistic app flows through the real wrapper and watch `JLINKARM_IsOpen`.
//!
//! Reproduces "autoconnect works but a later manual connect fails": the connect
//! sequence starts with SelectByUSBSN, which returns -1 whenever the DLL is
//! still open. So any flow that leaves the DLL open makes the next connect fail
//! at step one.
//!
//!     cargo run --example session_replay -- <device> [serial]

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::{JLinkDll, JLINKARM_TIF_SWD};
use std::time::Duration;

fn is_open(dll: &JLinkDll) -> String {
    // Read IsOpen through the same wrapper used by the app.
    if dll.is_open() { "1" } else { "0" }.to_string()
}

fn main() {
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let device = std::env::args().nth(1).unwrap_or_else(|| "STM32F030C8".into());
    let serial: Option<u32> = std::env::args().nth(2).and_then(|s| s.parse().ok());
    let dll = JLinkDll::load(&dll_path).expect("load");
    println!("DLL {}", dll.version_string());

    // The app's connect sequence, mirroring open_session, including the
    // stale-session reset that open_session now performs before selecting.
    let full_connect = |label: &str, reset_first: bool| {
        println!("\n=== {label} ===");
        println!("  before: IsOpen={}", is_open(&dll));
        if reset_first {
            // This is exactly what open_session::reset_session does.
            dll.rtt_stop();
            if dll.is_open() {
                dll.close();
            }
            println!("  after reset: IsOpen={}", is_open(&dll));
        }
        if let Some(sn) = serial {
            match dll.select_by_serial(sn) {
                Ok(()) => println!("  select ok"),
                Err(e) => {
                    println!("  select FAILED: {e:#}   <-- the reported error");
                    return;
                }
            }
        }
        match dll.open() {
            Ok(()) => println!("  open ok, IsOpen={}", is_open(&dll)),
            Err(e) => {
                println!("  open FAILED: {e:#}");
                return;
            }
        }
        let _ = dll.set_interface(JLINKARM_TIF_SWD);
        dll.set_speed(4000);
        let _ = dll.exec_command(&format!("device = {device}"));
        match dll.connect() {
            Ok(()) => println!("  connect ok"),
            Err(e) => println!("  connect FAILED: {e:#}"),
        }
        match dll.rtt_start(Duration::from_secs(5)) {
            Ok(()) => println!("  rtt_start ok, up={:?}", dll.rtt_num_up_buffers()),
            Err(e) => println!("  rtt_start FAILED: {e:#}"),
        }
    };

    let teardown = |label: &str| {
        println!("  -- teardown ({label}) --");
        dll.rtt_stop();
        dll.close();
        println!("  after teardown: IsOpen={}", is_open(&dll));
    };

    // The failing case: a session is left open, then connect runs again.
    // Without the reset this failed at select; with it, it must succeed.
    full_connect("connect #1 (clean start)", false);
    full_connect("connect #2 while session still OPEN (was the bug)", true);
    full_connect("connect #3 still open, reset again", true);

    teardown("cleanup");
    println!("\ndone");
}
