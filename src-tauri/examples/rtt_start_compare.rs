//! Regression check for `rtt_start` readiness detection.
//!
//! Pins down two facts about the J-Link DLL (V6.88a) that caused a real
//! "connection failed" report:
//!
//! 1. `GETSTAT.IsRunning` is 1 **immediately** after START, before any buffer
//!    exists — so it must not be used as the success signal.
//! 2. The up-buffer count converges a few hundred ms later, and is the signal
//!    `rtt_start` waits for.
//!
//! Run with a probe and target running RTT firmware:
//!
//!     cargo run --example rtt_start_compare -- <device> [serial]

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::{JLinkDll, JLINKARM_TIF_SWD};
use std::time::{Duration, Instant};

fn main() {
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let device = std::env::args().nth(1).unwrap_or_else(|| "STM32F030C8".into());
    let serial: Option<u32> = std::env::args().nth(2).and_then(|s| s.parse().ok());
    let dll = JLinkDll::load(&dll_path).expect("load");
    println!("DLL {} / device {device} / serial {serial:?}", dll.version_string());

    if let Some(sn) = serial {
        let _ = dll.select_by_serial(sn);
    }
    if let Err(e) = dll.open() {
        println!("open failed: {e:#}");
        return;
    }
    let _ = dll.set_interface(JLINKARM_TIF_SWD);
    dll.set_speed(4000);
    let _ = dll.exec_command(&format!("device = {device}"));
    if let Err(e) = dll.connect() {
        println!("connect failed: {e:#}");
        dll.close();
        return;
    }

    dll.rtt_stop();
    std::thread::sleep(Duration::from_millis(300));

    // Issue START and immediately sample both indicators.
    dll.rtt_start_once();
    let running_at_0 = dll.rtt_is_running();
    let num_at_0 = dll.rtt_num_up_buffers();
    println!("\nright after START:");
    println!("  IsRunning    = {running_at_0:?}   <- unreliable: 1 even with no buffers");
    println!("  num_up_buffers = {num_at_0:?}   <- reliable signal");

    dll.rtt_stop();
    std::thread::sleep(Duration::from_millis(300));

    // The real function must wait for the reliable signal.
    println!("\ncalling rtt_start(timeout=5s):");
    let started = Instant::now();
    match dll.rtt_start(Duration::from_secs(5)) {
        Ok(()) => {
            let took = started.elapsed();
            let n = dll.rtt_num_up_buffers();
            println!("  ok after {took:?}, num_up_buffers = {n:?}");
            let ups: Vec<i32> = dll.rtt_up_buffers().iter().map(|b| b.index).collect();
            println!("  up buffers = {ups:?}");
            if took < Duration::from_millis(100) {
                println!("  WARNING: returned almost immediately — may have accepted a false signal");
            }
            if n.unwrap_or(0) == 0 {
                println!("  WARNING: succeeded with 0 up buffers — readiness check is wrong");
            }
        }
        Err(e) => println!("  FAILED: {e:#}"),
    }

    dll.rtt_stop();
    dll.close();
}
