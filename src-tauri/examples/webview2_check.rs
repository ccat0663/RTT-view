//! Report what the WebView2 detection sees on this machine.
//!
//! The startup guard treats `Missing` as "block and explain", so this must agree
//! with reality: on a machine with the runtime installed it has to say
//! `Installed`, otherwise the app would refuse to start on a perfectly good PC.
//!
//!     cargo run --example webview2_check

fn main() {
    let status = rtt_view_lib::webview2_status();
    println!("detected: {status:?}");
    match status {
        rtt_view_lib::WebView2Status::Installed(v) => {
            println!("=> WebView2 present (version {v})");
        }
        rtt_view_lib::WebView2Status::Missing => {
            println!("=> NOT installed — the app would show the install prompt");
        }
        rtt_view_lib::WebView2Status::Unknown => {
            println!("=> unknown (app proceeds; the runtime will report its own error if absent)");
        }
    }
}
