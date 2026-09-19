//! Locating the J-Link DLL on disk.
//!
//! SEGGER does not install to a fixed path (and the host API headers ship only
//! with the paid SDK), so the DLL is found by probing the registry, then a
//! bounded filesystem scan, then a user-supplied path.
//!
//! The supported-device list is *not* read here: it lives in the DLL's built-in
//! database and is enumerated via [`crate::jlink::JLinkDll::list_devices`]. The
//! `JLinkDevices.xml` file next to the DLL only adds or overrides a few hundred
//! entries, so it is deliberately not used as the catalogue.

use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Find `JLink_x64.dll` (or `JLinkARM.dll` as a fallback).
///
/// Candidates are gathered from every registry hive *and* a filesystem scan,
/// then ranked together. Taking the first registry hit is not safe: machines
/// with several installs often have inconsistent `InstallPath` values across
/// the 64-bit, WOW6432Node and HKCU hives. Ranking everything by (64-bit,
/// newest) picks the most capable install even in that case, and the user can
/// always override with an explicit path.
pub fn find_jlink_dll(config_path: Option<&str>) -> Result<PathBuf> {
    // An explicit path always wins.
    if let Some(p) = config_path {
        if !p.is_empty() {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Ok(p);
            }
            bail!("指定的 DLL 路径不存在: {}", p.display());
        }
    }

    let mut candidates = registry_candidates();
    candidates.extend(scan_for_dlls());
    candidates.sort();
    candidates.dedup();

    if let Some(dll) = pick_best(candidates) {
        return Ok(dll);
    }

    bail!(
        "未找到 J-Link DLL（JLink_x64.dll）。\n\
         请安装 SEGGER J-Link 软件包，或在连接面板中手动指定 DLL 路径。"
    )
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

fn preferred_dll_in(dir: &Path) -> Option<PathBuf> {
    for name in ["JLink_x64.dll", "JLinkARM.dll"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn registry_candidates() -> Vec<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let probes = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\SEGGER\J-Link"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\SEGGER\J-Link"),
        (HKEY_CURRENT_USER, r"SOFTWARE\SEGGER\J-Link"),
    ];
    let mut out = Vec::new();
    for (hive, sub) in probes {
        if let Ok(key) = RegKey::predef(hive).open_subkey(sub) {
            if let Ok(v) = key.get_value::<String, _>("InstallPath") {
                let dir = PathBuf::from(v);
                if let Some(dll) = preferred_dll_in(&dir) {
                    out.push(dll);
                }
            }
        }
    }
    out
}

/// Bounded scan of every drive for a J-Link installation.
fn scan_for_dlls() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in b'C'..=b'Z' {
        let root = format!("{}:\\", d as char);
        if !Path::new(&root).exists() {
            continue;
        }
        for base in [
            "SEGGER",
            "Program Files\\SEGGER",
            "Program Files (x86)\\SEGGER",
        ] {
            scan_dir_for_dll(&Path::new(&root).join(base), &mut out);
        }
        // Root-level folders such as `D:\jlink` or `D:\JLink_V688a`.
        if let Ok(rd) = fs::read_dir(&root) {
            for entry in rd.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let lower = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if lower.starts_with("jlink") || lower.starts_with("segger") {
                    scan_dir_for_dll(&p, &mut out);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn scan_dir_for_dll(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    if let Some(dll) = preferred_dll_in(dir) {
        out.push(dll);
    }
    // One level down (versioned folders like `JLink_V794`).
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(dll) = preferred_dll_in(&p) {
                    out.push(dll);
                }
            }
        }
    }
}

/// Choose among multiple installations: prefer 64-bit DLLs, then the newest
/// install (by file mtime), so a machine with several J-Link versions picks the
/// most recent one instead of whichever sorts first alphabetically.
fn pick_best(mut candidates: Vec<PathBuf>) -> Option<PathBuf> {
    candidates.sort_by(|a, b| {
        let a64 = is_x64(a);
        let b64 = is_x64(b);
        // 64-bit first.
        b64.cmp(&a64)
            .then_with(|| mtime(b).cmp(&mtime(a))) // newest second
            .then_with(|| a.cmp(b)) // stable tiebreak
    });
    candidates.into_iter().next()
}

fn is_x64(p: &Path) -> bool {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase().contains("x64"))
        .unwrap_or(false)
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}
