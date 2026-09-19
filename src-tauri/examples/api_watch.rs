//! Which J-Link DLL calls are safe when no emulator is behind them?
//!
//! Why this exists: after a probe was unplugged mid-session, the app kept raising
//! the J-Link emulator / network-IP dialog, and dismissing it only brought it
//! back. The DLL is a process-wide singleton that the acquisition thread polls
//! continuously, so the question was which call makes it go looking for an
//! emulator — and, just as important, which calls are safe.
//!
//! Measured on J-Link V6.88a with no probe attached, in the state a disconnect
//! leaves behind (`Open` fails → `Close`):
//!
//! | call                          | result                                      |
//! |-------------------------------|---------------------------------------------|
//! | `JLINKARM_IsOpen`             | returns false, safe                         |
//! | `JLINKARM_Open`               | returns "Cannot connect to J-Link", safe    |
//! | `JLINKARM_Close`              | returns, safe                               |
//! | `JLINKARM_IsConnected`        | **blocks**                                  |
//! | `JLINK_RTTERMINAL_Control(*)` | **blocks**: STOP, GETSTAT, GETNUMBUF, READ  |
//!
//! The blocking calls also spawn `JLinkGUIServer.exe` and raise the IP dialog, so
//! no RTT command — not even STOP — may be issued once the link is gone. That is
//! what `rtt_query_allowed` in `acquisition.rs` enforces.
//!
//! `control` and `probe` are the decisive pair: identical except for the final RTT
//! call, and `control` is the proof that the harness itself is not the cause.
//!
//!     cargo run --example api_watch -- control   # open -> close -> done
//!     cargo run --example api_watch -- probe     # open -> close -> GETSTAT (hangs)
//!     cargo run --example api_watch -- safe      # the predicates, for contrast
//!
//! Wrap every run in a timeout: a blocking call hangs the process by design, so
//! the timeout expiring *is* the result.

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::JLinkDll;
use std::process::Command;

/// `JLinkGUIServer.exe` is the usual host of the emulator/IP dialog.
fn gui_server_count() -> usize {
    match Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq JLinkGUIServer.exe", "/FO", "CSV", "/NH"])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .matches("JLinkGUIServer.exe")
            .count(),
        Err(_) => 0,
    }
}

/// Put the DLL into the state a disconnect leaves: a failed open, then close.
fn dead_session(dll: &JLinkDll) {
    let r = dll.open();
    eprintln!("[watch] open() -> {:?}", r.map(|_| "ok"));
    dll.close();
    eprintln!("[watch] close() returned; is_open={}", dll.is_open());
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let dll = JLinkDll::load(&dll_path).expect("load failed");
    eprintln!("[watch] mode={mode} dll={}", dll_path.display());
    eprintln!("[watch] baseline: gui_servers={}", gui_server_count());

    match mode.as_str() {
        // Control arm: the same setup, no RTT call. Returns immediately.
        "control" => {
            dead_session(&dll);
            eprintln!("[watch] DONE (no RTT call made), gui_servers={}", gui_server_count());
        }
        // Test arm: differs only by the RTT call, which blocks and raises the dialog.
        "probe" => {
            dead_session(&dll);
            eprintln!("[watch] rtt_status() [GETSTAT] ...");
            let s = dll.rtt_status();
            eprintln!("[watch] rtt_status -> {}", if s.is_some() { "Some" } else { "None" });
            eprintln!("[watch] DONE, gui_servers={}", gui_server_count());
        }
        // The safe calls, for contrast: this must return promptly.
        "safe" => {
            dead_session(&dll);
            eprintln!("[watch] is_open() -> {}", dll.is_open());
            eprintln!("[watch] DONE (safe calls returned)");
        }
        _ => {
            eprintln!("usage: api_watch <control|probe|safe>");
        }
    }
}
