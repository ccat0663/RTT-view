//! WebView2 runtime detection.
//!
//! The app needs the WebView2 runtime to render its UI. Windows 10 1803+ and
//! Windows 11 normally ship it, but trimmed-down or server editions may not, and
//! without it the webview cannot be created at all.
//!
//! Detecting it before startup lets us show an actionable Chinese message
//! instead of letting the process die silently — which is what happens
//! otherwise, because the release profile sets `panic = "abort"`.
//!
//! Detection follows Microsoft's documented method: read the `pv` value under
//! `...\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`.
//! The key lives under `WOW6432Node` on 64-bit systems (the runtime is 32-bit
//! even on x64) and directly under `SOFTWARE` on 32-bit systems; per-user
//! installs use HKCU.

/// WebView2's EdgeUpdate client GUID (per Microsoft's documentation).
const WEBVIEW2_CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// Whether a raw `pv` registry value means "installed".
///
/// Microsoft's contract: a missing value, an empty string, or `0.0.0.0` all mean
/// the runtime is not installed. Kept pure so the rule is unit-testable.
pub fn version_is_installed(pv: Option<&str>) -> bool {
    match pv {
        None => false,
        Some(v) => {
            let t = v.trim();
            !t.is_empty() && t != "0.0.0.0"
        }
    }
}

/// Outcome of probing the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebView2Status {
    /// Found, with the reported version.
    Installed(String),
    /// Definitively absent (keys read fine but hold no usable version).
    Missing,
    /// Could not tell (e.g. access denied). Callers should proceed and let the
    /// runtime report its own error rather than blocking a possibly-fine setup.
    Unknown,
}

/// Read the `pv` value from one registry location.
///
/// `hive` is a predefined root handle (`winreg::enums::HKEY_*`, typed as a raw
/// pointer). `Ok(None)` means the key or value is absent (a normal "not
/// installed here"); `Err(())` means the query itself failed for another reason.
#[cfg(target_os = "windows")]
fn read_pv(
    hive: *mut std::ffi::c_void,
    subkey: &str,
) -> Result<Option<String>, ()> {
    use winreg::enums::KEY_READ;
    use winreg::RegKey;

    let key = match RegKey::predef(hive).open_subkey_with_flags(subkey, KEY_READ) {
        Ok(k) => k,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    match key.get_value::<String, _>("pv") {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// Probe the machine for the WebView2 runtime.
#[cfg(target_os = "windows")]
pub fn detect() -> WebView2Status {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    // 64-bit systems keep the (32-bit) runtime under WOW6432Node; 32-bit systems
    // do not. Query both, plus HKCU for per-user installs.
    let locations = [
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT_GUID}"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT_GUID}"),
        ),
        (
            HKEY_CURRENT_USER,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_CLIENT_GUID}"),
        ),
    ];

    let mut any_read_ok = false;
    for (hive, subkey) in locations {
        match read_pv(hive, &subkey) {
            Ok(Some(v)) if version_is_installed(Some(&v)) => {
                return WebView2Status::Installed(v);
            }
            Ok(_) => any_read_ok = true,
            Err(()) => {}
        }
    }

    if any_read_ok {
        WebView2Status::Missing
    } else {
        WebView2Status::Unknown
    }
}

#[cfg(not(target_os = "windows"))]
pub fn detect() -> WebView2Status {
    WebView2Status::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_installed_accepts_real_versions() {
        assert!(version_is_installed(Some("153.0.4234.32")));
        assert!(version_is_installed(Some(" 120.0.0.1 ")));
        assert!(version_is_installed(Some("1.0.0.0")));
    }

    #[test]
    fn version_is_installed_rejects_absent_and_zero() {
        assert!(!version_is_installed(None));
        assert!(!version_is_installed(Some("")));
        assert!(!version_is_installed(Some("   ")));
        assert!(!version_is_installed(Some("0.0.0.0")));
        assert!(!version_is_installed(Some(" 0.0.0.0 ")));
    }
}
