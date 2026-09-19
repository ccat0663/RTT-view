//! Verify RTT up-buffer enumeration against the installed J-Link DLL.
//!
//! `JLINKARM_EMU_GetList` and `JLINKARM_DEVICE_GetInfo` are covered by
//! `probe_check`; this example checks the RTT side added for "capture all
//! channels": `JLINK_RTTERMINAL_Control(GETNUMBUF, DIR_UP)` must return a count,
//! and `GETDESC` must accept index+direction.
//!
//! It opens a probe and starts RTT, so it needs a connected target running RTT
//! firmware. Without one it still exercises the code path and reports what the
//! DLL returned, which is the useful signal for a stack with no hardware.
//!
//!     cargo run --example rtt_channels

use rtt_view_lib::discovery;
use rtt_view_lib::jlink::JLinkDll;
use std::time::Duration;

fn main() {
    let dll_path = discovery::find_jlink_dll(None).expect("DLL not found");
    println!("DLL: {}", dll_path.display());
    let dll = JLinkDll::load(&dll_path).expect("load failed");
    println!("版本: {}", dll.version_string());

    let probes = dll.list_probes().unwrap_or_default();
    println!("调试器: {} 个", probes.len());
    if probes.is_empty() {
        println!("未检测到调试器，仅能验证 API 调用不崩溃。");
        return;
    }

    // Enumerating buffers before RTT is started is expected to fail gracefully.
    println!("\n[未启动 RTT] GETNUMBUF(DIR_UP) = {:?}", dll.rtt_num_up_buffers());
    println!("[未启动 RTT] up buffers = {} 个", dll.rtt_up_buffers().len());

    match dll.open() {
        Ok(()) => println!("\nOpen: ok"),
        Err(e) => {
            println!("\nOpen 失败（可能被占用或目标未上电）: {e:#}");
            return;
        }
    }
    if let Err(e) = dll.set_interface(rtt_view_lib::jlink::JLINKARM_TIF_SWD) {
        println!("TIF 失败: {e:#}");
    }

    // rtt_start needs a valid device name; skip it if we can't connect.
    match dll.rtt_start(Duration::from_secs(3)) {
        Ok(()) => {
            let n = dll.rtt_num_up_buffers();
            println!("\n[已启动 RTT] GETNUMBUF(DIR_UP) = {n:?}");
            let buffers = dll.rtt_up_buffers();
            println!("up buffers = {} 个", buffers.len());
            for b in &buffers {
                println!("  ch{} dir={} size={} name={:?}", b.index, b.direction, b.size, b.name);
            }
            if buffers.is_empty() {
                println!("（目标未运行 RTT 固件时属正常：控制块找不到）");
            }
            dll.rtt_stop();
        }
        Err(e) => println!("\nrtt_start 未成功（目标未运行 RTT 固件时正常）: {e:#}"),
    }
    dll.close();
}
