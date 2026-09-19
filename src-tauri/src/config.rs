//! Persisted user settings (last-used connection parameters, UI toggles).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Connection + UI settings remembered across launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Explicit DLL path (empty = auto-detect).
    pub dll_path: String,
    /// Probe serial number to use (0 = auto / single probe).
    pub probe_serial: u32,
    /// Target device name, e.g. `STM32F407VG`.
    pub device: String,
    /// Target interface: `SWD` or `JTAG`.
    pub interface: String,
    /// Interface speed in kHz (0 = auto).
    pub speed_khz: u32,
    /// RTT up channel index.
    pub up_channel: u32,
    /// Capture every RTT up channel instead of only `up_channel`. Useful when the
    /// target's channel is unknown, at the cost of polling more buffers.
    pub capture_all: bool,
    /// Reset the target (and let it run) as part of connecting.
    ///
    /// This mirrors Keil's "Reset and Run" after a download: it puts the target
    /// into a known state, which matters when the previous tool left the CPU
    /// halted at the reset vector so the firmware never started.
    pub reset_on_connect: bool,
    /// Auto-connect on launch when a probe is present.
    pub auto_connect: bool,
    /// Show the calendar date in the log view's timestamp.
    pub show_date: bool,
    /// Show the wall-clock time in the log view's timestamp. Date and time are
    /// independent, so either can be displayed alone.
    pub show_time: bool,
    /// Show the monotonic +seconds offset alongside wall time.
    pub show_relative: bool,
    /// Keep the log view pinned to the bottom unless the user scrolls up.
    pub autoscroll: bool,
    /// Log view font size in px, chosen from a fixed set of steps.
    pub log_font_size: f64,
    /// Max lines retained in the log view.
    pub max_lines: usize,
    /// Save format: `timestamp` | `raw` | `csv`.
    pub save_format: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dll_path: String::new(),
            probe_serial: 0,
            device: String::new(),
            interface: "SWD".into(),
            speed_khz: 4000,
            up_channel: 0,
            capture_all: false,
            reset_on_connect: true,
            auto_connect: false,
            show_date: true,
            show_time: true,
            show_relative: true,
            autoscroll: true,
            log_font_size: 12.5,
            max_lines: 5000,
            save_format: "timestamp".into(),
        }
    }
}

fn settings_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("settings.json")
}

pub fn load(app_config_dir: &Path) -> Settings {
    let p = settings_path(app_config_dir);
    match std::fs::read_to_string(&p) {
        Ok(s) => parse(&s),
        Err(_) => Settings::default(),
    }
}

/// Parse settings text, upgrading files written before the timestamp toggle was
/// split into separate date and time toggles.
fn parse(text: &str) -> Settings {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Settings::default();
    };
    migrate_timestamp_toggles(&mut value);
    serde_json::from_value(value).unwrap_or_default()
}

/// Carry a legacy `showTimestamp` over to `showDate` + `showTime`.
///
/// Without this the old key is simply unknown, both new fields fall back to their
/// default of `true`, and a user who had timestamps switched *off* would find them
/// switched back on. Applied only when neither new key is present, so it can never
/// override a file the current version wrote.
fn migrate_timestamp_toggles(value: &mut serde_json::Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    if obj.contains_key("showDate") || obj.contains_key("showTime") {
        return;
    }
    if let Some(legacy) = obj.get("showTimestamp").cloned() {
        obj.insert("showDate".into(), legacy.clone());
        obj.insert("showTime".into(), legacy);
    }
}

pub fn save(app_config_dir: &Path, settings: &Settings) -> Result<()> {
    std::fs::create_dir_all(app_config_dir)
        .with_context(|| format!("无法创建配置目录 {}", app_config_dir.display()))?;
    let p = settings_path(app_config_dir);
    let text = serde_json::to_string_pretty(settings)?;
    std::fs::write(&p, text).with_context(|| format!("无法写入 {}", p.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_timestamp_toggle_migrates_onto_both_new_ones() {
        // A file from before the split: timestamps were off.
        let s = parse(r#"{"showTimestamp": false, "showRelative": false}"#);
        assert!(!s.show_date);
        assert!(!s.show_time);
        assert!(!s.show_relative);
    }

    #[test]
    fn legacy_timestamp_toggle_migrates_when_enabled() {
        let s = parse(r#"{"showTimestamp": true}"#);
        assert!(s.show_date);
        assert!(s.show_time);
    }

    #[test]
    fn migration_never_overrides_the_new_keys() {
        // A current-version file stays exactly as written, even with the old key
        // still lying around.
        let s = parse(r#"{"showDate": false, "showTime": true, "showTimestamp": false}"#);
        assert!(!s.show_date);
        assert!(s.show_time);
    }

    #[test]
    fn a_file_without_a_font_size_keeps_the_default() {
        // Files written before the font-size control existed must still load, and
        // land on the log's original 12.5px rather than on 0.
        let s = parse(r#"{"autoscroll": true}"#);
        assert_eq!(s.log_font_size, 12.5);
    }

    #[test]
    fn the_font_size_survives_a_round_trip() {
        let s = Settings {
            log_font_size: 15.5,
            ..Settings::default()
        };
        let text = serde_json::to_string_pretty(&s).unwrap();
        assert_eq!(parse(&text).log_font_size, 15.5);
    }

    #[test]
    fn missing_keys_keep_the_defaults_of_both_on() {
        let s = parse("{}");
        assert!(s.show_date && s.show_time);
        assert!(s.autoscroll);
    }

    #[test]
    fn corrupt_settings_fall_back_to_defaults() {
        let s = parse("not json at all");
        assert!(s.show_date && s.show_time);
    }

    #[test]
    fn settings_round_trip_through_the_file_format() {
        let s = Settings {
            show_date: false,
            device: "STM32F030C8".into(),
            ..Settings::default()
        };
        let text = serde_json::to_string_pretty(&s).unwrap();
        let back = parse(&text);
        assert!(!back.show_date);
        assert!(back.show_time);
        assert_eq!(back.device, "STM32F030C8");
    }
}
