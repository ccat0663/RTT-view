//! Verify `JLINKARM_ResetNoHalt` resets the target and RTT recovers.
//!
//! Confirms the assumption behind the 复位 button: after a no-halt reset the CPU
//! is running (not halted) and RTT re-arms on its own, so the capture loop keeps
//! working. The firmware under test prints a counter, so a visible restart of the
//! counter is direct evidence the reset took effect.
//!
//!     cargo run --example reset_check -- <device> [serial]

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::{JLinkDll, JLINKARM_TIF_SWD};
use std::time::{Duration, Instant};

fn main() {
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let device = std::env::args().nth(1).unwrap_or_else(|| "STM32F030C8".into());
    let serial: Option<u32> = std::env::args().nth(2).and_then(|s| s.parse().ok());
    let dll = JLinkDll::load(&dll_path).expect("load");
    println!("DLL {} device {device} serial {serial:?}", dll.version_string());

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
    if dll.is_halted() {
        dll.go();
    }
    if let Err(e) = dll.rtt_start(Duration::from_secs(5)) {
        println!("rtt_start failed: {e:#}");
        dll.close();
        return;
    }
    println!("connected; up buffers = {:?}", dll.rtt_up_buffers().iter().map(|b| b.index).collect::<Vec<_>>());

    // Read a little to show the counter before reset.
    let read_for = |label: &str, ms: u64| -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            for ch in 0..4u32 {
                if let Ok(n) = dll.rtt_read(ch, &mut buf) {
                    if n > 0 {
                        let t = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                        if !t.is_empty() {
                            out.push(t);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("  [{label}] {} lines, last: {:?}", out.len(), out.last());
        out
    };

    println!("\n-- before reset --");
    let before = read_for("before", 1500);
    println!("  halted = {}", dll.is_halted());

    println!("\n-- RESET (ResetNoHalt) --");
    let t0 = Instant::now();
    dll.rtt_stop();
    dll.reset_no_halt();
    println!("  reset returned after {:?}", t0.elapsed());
    println!("  halted immediately after reset = {}", dll.is_halted());

    // RTT control block must reappear once firmware re-inits.
    println!("\n-- re-arm RTT --");
    let t1 = Instant::now();
    match dll.rtt_start(Duration::from_secs(5)) {
        Ok(()) => println!("  rtt_start ok after {:?}, up buffers = {:?}",
            t1.elapsed(), dll.rtt_up_buffers().iter().map(|b| b.index).collect::<Vec<_>>()),
        Err(e) => {
            println!("  rtt_start FAILED: {e:#}");
            dll.close();
            return;
        }
    }

    println!("\n-- after reset --");
    let after = read_for("after", 2000);
    println!("  halted = {}", dll.is_halted());

    // The firmware prints an incrementing counter. If the reset worked, the
    // counter restarts, so the first post-reset value should be near zero.
    let num = |s: &str| -> Option<u64> {
        // The counter is the last run of digits in the line.
        s.rsplit(|c: char| !c.is_ascii_digit())
            .find(|x| !x.is_empty())
            .and_then(|x| x.parse().ok())
    };
    let last_before = before.last().and_then(|s| num(s));
    let first_after = after.first().and_then(|s| num(s));
    println!("\n  counter before reset: {last_before:?}");
    println!("  counter after reset : {first_after:?}");
    match (last_before, first_after) {
        (Some(b), Some(a)) if a < b => println!("  => counter restarted, reset confirmed"),
        (Some(_), Some(_)) => println!("  => counter did NOT restart (check manually)"),
        _ => println!("  => could not parse counters (no data?)"),
    }

    dll.rtt_stop();
    dll.close();
}
