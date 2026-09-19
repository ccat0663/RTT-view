//! Tauri commands: the GUI's entry points into the backend.
//!
//! Commands are intentionally thin. Anything touching the J-Link DLL is handed
//! to the acquisition thread (which owns the DLL's single-threaded contract);
//! results come back asynchronously on the IPC channel.

use crate::acquisition::{self, AcqCommand, AcqEvent, ConnectConfig};
use crate::config::Settings;
use crate::logging::SaveFormat;
use crate::state::AppState;
use serde::Deserialize;
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::State;

/// Start the acquisition thread and bind the frontend's event channel.
///
/// Must be called once, before any other command.
#[tauri::command]
pub fn start_acquisition(
    channel: Channel<AcqEvent>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.acq.lock().map_err(|_| "锁已损坏".to_string())?.is_some() {
        return Ok(()); // already running
    }
    let handle = acquisition::spawn(channel);
    state.set_acq(handle);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectArgs {
    pub dll_path: String,
    pub probe_serial: u32,
    pub device: String,
    pub interface: String,
    pub speed_khz: u32,
    pub up_channel: u32,
    #[serde(default)]
    pub capture_all: bool,
    #[serde(default = "default_true")]
    pub reset_on_connect: bool,
}

fn default_true() -> bool {
    true
}

impl ConnectArgs {
    fn to_config(&self) -> ConnectConfig {
        ConnectConfig {
            dll_path: self.dll_path.clone(),
            probe_serial: self.probe_serial,
            device: self.device.clone(),
            interface: self.interface.clone(),
            speed_khz: self.speed_khz,
            up_channel: self.up_channel,
            capture_all: self.capture_all,
            reset_on_connect: self.reset_on_connect,
        }
    }

    fn remember(&self, state: &AppState) {
        if let Ok(mut s) = state.settings.lock() {
            s.dll_path = self.dll_path.clone();
            s.probe_serial = self.probe_serial;
            s.device = self.device.clone();
            s.interface = self.interface.clone();
            s.speed_khz = self.speed_khz;
            s.up_channel = self.up_channel;
            s.capture_all = self.capture_all;
            s.reset_on_connect = self.reset_on_connect;
        }
        state.persist();
    }
}

#[tauri::command]
pub fn connect(args: ConnectArgs, state: State<'_, AppState>) -> Result<(), String> {
    args.remember(&state);
    state.acq()?.send(AcqCommand::Connect(Box::new(args.to_config())))
}

#[tauri::command]
pub fn test_connection(args: ConnectArgs, state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::Test(Box::new(args.to_config())))
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::Disconnect)
}

/// Reset the target over J-Link and leave it running.
#[tauri::command]
pub fn reset_target(state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::Reset)
}

/// Clear the log view and reset the line counter.
#[tauri::command]
pub fn clear_log(state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::ClearLog)
}

#[tauri::command]
pub fn refresh_probes(dll_path: String, state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::RefreshProbes { dll_path })
}

#[tauri::command]
pub fn refresh_devices(dll_path: String, state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::RefreshDevices { dll_path })
}

#[tauri::command]
pub fn validate_device(name: String, state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::ValidateDevice { name })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveArgs {
    pub path: String,
    pub format: String,
    pub append: bool,
}

#[tauri::command]
pub fn start_save(args: SaveArgs, state: State<'_, AppState>) -> Result<(), String> {
    if let Ok(mut s) = state.settings.lock() {
        s.save_format = args.format.clone();
    }
    state.persist();
    state.acq()?.send(AcqCommand::StartSave {
        path: PathBuf::from(args.path),
        format: SaveFormat::parse(&args.format),
        append: args.append,
    })
}

#[tauri::command]
pub fn stop_save(state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::StopSave)
}

#[tauri::command]
pub fn set_display(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.acq()?.send(AcqCommand::SetDisplay(enabled))
}

/// Read persisted settings so the UI can pre-fill its controls.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    state
        .settings
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "设置锁已损坏".to_string())
}

/// Update individual UI preferences and persist them.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_prefs(
    show_date: Option<bool>,
    show_time: Option<bool>,
    show_relative: Option<bool>,
    autoscroll: Option<bool>,
    log_font_size: Option<f64>,
    max_lines: Option<usize>,
    auto_connect: Option<bool>,
    reset_on_connect: Option<bool>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Ok(mut s) = state.settings.lock() {
        if let Some(v) = show_date {
            s.show_date = v;
        }
        if let Some(v) = show_time {
            s.show_time = v;
        }
        if let Some(v) = show_relative {
            s.show_relative = v;
        }
        if let Some(v) = autoscroll {
            s.autoscroll = v;
        }
        if let Some(v) = log_font_size {
            // The UI sends one of five steps; clamping here keeps a hand-edited
            // settings file from producing an unreadable log. Non-finite input is
            // dropped rather than persisted.
            if v.is_finite() {
                s.log_font_size = v.clamp(8.0, 72.0);
            }
        }
        if let Some(v) = max_lines {
            s.max_lines = v.clamp(100, 500_000);
        }
        if let Some(v) = auto_connect {
            s.auto_connect = v;
        }
        if let Some(v) = reset_on_connect {
            s.reset_on_connect = v;
        }
    }
    state.persist();
    Ok(())
}
