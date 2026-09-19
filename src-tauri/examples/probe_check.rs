//! Verify the device database can be enumerated from the installed J-Link DLL.
//!
//! Run with `cargo run --example probe_check` from `src-tauri/`. This is the
//! end-to-end proof for the chip picker: it loads the real DLL, walks the
//! built-in device database through `JLINKARM_DEVICE_GetInfo`, and prints the
//! count plus sample rows in the same shape the UI table shows.
//!
//! No probe or target is needed, and nothing is opened, so it is safe to run
//! while other J-Link software is in use.

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::JLinkDll;

/// Same rendering as the picker's RAM/Flash column: rounded KB, `-` for zero.
fn kb(n: u64) -> String {
    if n == 0 {
        "-".to_string()
    } else if n < 1024 {
        format!("{n} B")
    } else {
        format!("{} KB", (n as f64 / 1024.0).round() as u64)
    }
}

fn main() {
    println!("== J-Link discovery ==");

    let dll_path = match discovery::find_jlink_dll(None) {
        Ok(p) => p,
        Err(e) => {
            println!("DLL 未找到: {e:#}");
            std::process::exit(1);
        }
    };
    println!("DLL: {}", dll_path.display());

    let dll = match JLinkDll::load(&dll_path) {
        Ok(d) => d,
        Err(e) => {
            println!("DLL 加载失败: {e:#}");
            std::process::exit(1);
        }
    };
    println!("版本: {} (0x{:06X})", dll.version_string(), dll.version());

    match dll.list_probes() {
        Ok(probes) if probes.is_empty() => println!("调试器: 未检测到"),
        Ok(probes) => println!("调试器: 检测到 {} 个", probes.len()),
        Err(e) => println!("调试器枚举失败: {e:#}"),
    }

    println!("\n== Device database ==");
    let started = std::time::Instant::now();
    let devices = dll.list_devices();
    let elapsed = started.elapsed();
    println!("枚举到 {} 个设备，用时 {:.0?}", devices.len(), elapsed);

    if devices.is_empty() {
        println!("(设备库为空 —— 说明结构体布局与 DLL 不匹配)");
        std::process::exit(1);
    }

    // Show rows in the picker's column layout.
    println!("\n{:<22} {:<28} {:<12} {:>10} {:>10}", "厂商", "设备", "内核", "RAM", "Flash");
    for d in devices.iter().filter(|d| d.name == "STM32F407VG" || d.name == "nRF52840_xxAA" || d.name == "STM32F103C8").take(5) {
        println!(
            "{:<22} {:<28} {:<12} {:>10} {:>10}",
            d.vendor, d.name, d.core, kb(d.ram_bytes), kb(d.flash_bytes)
        );
    }
    println!("\n前 5 条：");
    for d in devices.iter().take(5) {
        println!(
            "{:<22} {:<28} {:<12} {:>10} {:>10}",
            d.vendor, d.name, d.core, kb(d.ram_bytes), kb(d.flash_bytes)
        );
    }

    // Sanity: the DLL's own lookup must agree with the enumeration.
    match dll.device_index("STM32F407VG") {
        Ok(idx) => println!("\nDEVICE_GetIndex(\"STM32F407VG\") = {idx}"),
        Err(e) => println!("\nDEVICE_GetIndex 失败: {e:#}"),
    }
}
