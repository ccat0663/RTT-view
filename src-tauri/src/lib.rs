//! RTT View backend: host timestamps for SEGGER RTT output via the J-Link DLL.

mod acquisition;
mod commands;
mod config;
mod dialog;
pub mod discovery;
pub mod jlink;
pub mod line_assembler;
mod logging;
mod state;
pub mod webview2;

pub use webview2::WebView2Status;

/// Probe for the WebView2 runtime (exposed for diagnostics/verification).
pub fn webview2_status() -> WebView2Status {
    webview2::detect()
}

use state::AppState;
use tauri::Manager;

/// Explain a missing WebView2 runtime and point at the download.
const WEBVIEW2_HELP: &str = "\
本程序需要 Microsoft Edge WebView2 运行时才能显示界面，但当前系统未检测到它。

Windows 10（1803 及以上）和 Windows 11 通常已自带；精简版或服务器版系统可能缺失。

请安装后重试（免费）：
https://developer.microsoft.com/microsoft-edge/webview2/

安装选择「Evergreen Standalone Installer」即可。";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Check before building the webview: without the runtime the window cannot
    // be created, and in release (`panic = "abort"`, no console) the process
    // would otherwise exit with no explanation at all. `Unknown` is treated as
    // "probably fine" so a registry quirk never blocks a working machine.
    if matches!(webview2::detect(), webview2::WebView2Status::Missing) {
        dialog::show_error("RTT View — 缺少 WebView2 运行时", WEBVIEW2_HELP);
        return;
    }

    let built = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Show the version in the window title. Taken from the Cargo package
            // version so it cannot drift from the built artifact.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("RTT View v{}", env!("CARGO_PKG_VERSION")));
            }

            // Settings live in the OS-standard per-app config directory.
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let settings = config::load(&config_dir);
            app.manage(AppState::new(settings, config_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_acquisition,
            commands::connect,
            commands::test_connection,
            commands::disconnect,
            commands::reset_target,
            commands::clear_log,
            commands::refresh_probes,
            commands::refresh_devices,
            commands::validate_device,
            commands::start_save,
            commands::stop_save,
            commands::set_display,
            commands::get_settings,
            commands::set_prefs,
        ])
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(e) => {
            // Startup can fail for reasons other than a missing WebView2 (the
            // registry check above covers the common case). Report it rather than
            // dying silently, since release has no console.
            dialog::show_error(
                "RTT View — 启动失败",
                &format!("{e}\n\n若提示与 WebView2 有关，请安装运行时后重试：\n{WEBVIEW2_HELP}"),
            );
            return;
        }
    };

    app.run(|app_handle, event| {
        // On exit, stop the acquisition thread and wait for it to finish. That
        // thread owns the J-Link DLL; letting the process terminate while it is
        // still executing inside the DLL can deadlock shutdown, which leaves the
        // app running in the background instead of exiting.
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                // Bounded: a J-Link call stuck inside the DLL must not hold the
                // exit path open (see AcqHandle::shutdown).
                state.shutdown_acq(std::time::Duration::from_millis(1500));
            }
        }
    });
}
