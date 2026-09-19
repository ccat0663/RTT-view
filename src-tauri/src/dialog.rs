//! Native message boxes for startup failures.
//!
//! The app is built with `windows_subsystem = "windows"` (no console) and, in
//! release, `panic = "abort"` — so before the webview exists there is nowhere to
//! print a message: anything written to stderr is invisible and an `expect()`
//! just makes the process vanish. A native message box is the only way to tell
//! the user what went wrong, e.g. that the WebView2 runtime is missing.

/// Show a modal error dialog and wait for the user to dismiss it.
#[cfg(target_os = "windows")]
pub fn show_error(title: &str, message: &str) {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
    };

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(once(0)).collect()
    }

    let text = wide(message);
    let caption = wide(title);
    unsafe {
        MessageBoxW(
            0 as HWND,
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Non-Windows fallback so the module still compiles cross-platform.
#[cfg(not(target_os = "windows"))]
pub fn show_error(title: &str, message: &str) {
    eprintln!("{title}: {message}");
}
