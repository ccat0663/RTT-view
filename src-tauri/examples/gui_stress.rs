//! A/B stress test: does probe enumeration spawn the J-Link GUI dialog?
//!
//! `JLinkDll::load` now applies `SuppressGUI 1` etc. To compare, the `off` mode
//! turns suppression back off via the documented `SuppressGUI 0` switch, so both
//! arms use the exact same code path and only the flag differs.
//!
//!     cargo run --example gui_stress -- on       # fixed behaviour
//!     cargo run --example gui_stress -- off      # reproduces the old behaviour
//!
//! `JLinkGUIServer.exe` is the process that hosts the "Emulator selection"
//! dialog, so its appearance is the signal we count. Run the `off` arm under a
//! shell timeout: a modal dialog will block enumeration.
//!
//!     timeout 60 cargo run --example gui_stress -- off

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::JLinkDll;
use std::process::Command;

/// How many GUI-server processes are running right now.
fn gui_server_count() -> usize {
    let out = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq JLinkGUIServer.exe", "/FO", "CSV", "/NH"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.matches("JLinkGUIServer.exe").count()
        }
        Err(_) => 0,
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "on".into());
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    let dll = JLinkDll::load(&dll_path).expect("load failed");
    println!("[{mode}] loaded {}", dll_path.display());

    if mode == "off" {
        for cmd in ["SuppressGUI 0", "SuppressEmuUSBDialog 0", "HideDeviceSelection 0"] {
            let _ = dll.exec_command(cmd);
        }
        println!("[{mode}] suppression disabled");
    }

    println!("[{mode}] GUIServer before: {}", gui_server_count());

    let rounds = 30;
    let mut sightings = 0;
    for i in 0..rounds {
        let r = dll.list_probes();
        if gui_server_count() > 0 {
            sightings += 1;
            if sightings == 1 {
                println!("[{mode}] GUIServer appeared at round {i}");
            }
        }
        if i % 10 == 0 {
            println!("[{mode}] round {i}: probes={:?} gui_servers={}", r.map(|v| v.len()), gui_server_count());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    println!(
        "[{mode}] RESULT: {sightings}/{rounds} rounds had a GUI server; final GUIServer count = {}",
        gui_server_count()
    );
}
