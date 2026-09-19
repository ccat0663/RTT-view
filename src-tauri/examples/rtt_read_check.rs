//! Read actual RTT data for a few seconds — the end-to-end acceptance test for
//! "connection failed": connecting is only useful if bytes arrive.
//!
//!     cargo run --example rtt_read_check -- <device> [serial] [seconds]

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::{JLinkDll, JLINKARM_TIF_SWD};
use std::time::{Duration, Instant};

fn main() {
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let device = std::env::args().nth(1).unwrap_or_else(|| "STM32F030C8".into());
    let serial: Option<u32> = std::env::args().nth(2).and_then(|s| s.parse().ok());
    let secs: u64 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(4);

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
    // JLINKARM_Connect leaves the core halted, which stops the firmware from
    // printing. Report and resume, as RTT Viewer effectively does.
    println!("after connect: is_halted = {}", dll.is_halted());
    if dll.is_halted() {
        dll.go();
        std::thread::sleep(Duration::from_millis(200));
        println!("after Go:      is_halted = {}", dll.is_halted());
    }

    match dll.rtt_start(Duration::from_secs(5)) {
        Ok(()) => println!("rtt_start ok, up buffers = {:?}",
            dll.rtt_up_buffers().iter().map(|b| b.index).collect::<Vec<_>>()),
        Err(e) => {
            println!("rtt_start FAILED: {e:#}");
            dll.close();
            return;
        }
    }

    // Poll every up buffer and print whatever arrives.
    println!("\nreading for {secs}s (one poll loop per up buffer)...");
    let deadline = Instant::now() + Duration::from_secs(secs);
    let ups: Vec<u32> = dll.rtt_up_buffers().iter().map(|b| b.index as u32).collect();
    let mut buf = [0u8; 4096];
    let mut total = 0usize;
    let mut per_ch: std::collections::BTreeMap<u32, usize> = Default::default();
    while Instant::now() < deadline {
        for &ch in &ups {
            match dll.rtt_read(ch, &mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    total += n;
                    *per_ch.entry(ch).or_default() += n;
                    let text = String::from_utf8_lossy(&buf[..n]);
                    print!("[ch{ch}] {text}");
                }
                Err(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("\n\nreceived {total} bytes total; per channel: {per_ch:?}");
    if total == 0 {
        println!("NOTE: no bytes arrived. The connection succeeded but the firmware");
        println!("      may not be printing, or is writing to a different channel.");
    }

    dll.rtt_stop();
    dll.close();
}
