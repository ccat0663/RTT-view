//! Shared application state managed by Tauri.
//!
//! The acquisition thread is spawned lazily: it needs the frontend's IPC
//! `Channel`, which only exists once the web view has loaded. Until then the
//! handle is `None` and DLL-touching commands report a clear error.

use crate::acquisition::AcqHandle;
use crate::config::Settings;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub acq: Mutex<Option<AcqHandle>>,
    pub config_dir: PathBuf,
}

impl AppState {
    pub fn new(settings: Settings, config_dir: PathBuf) -> Self {
        Self { settings: Mutex::new(settings), acq: Mutex::new(None), config_dir }
    }

    /// Get a clone of the acquisition handle, or an explanatory error.
    pub fn acq(&self) -> Result<AcqHandle, String> {
        self.acq
            .lock()
            .map_err(|_| "采集线程句柄锁已损坏".to_string())?
            .clone()
            .ok_or_else(|| "采集线程尚未启动".to_string())
    }

    /// Stop the acquisition thread and wait for it to finish.
    ///
    /// Called on exit. The thread owns the J-Link DLL; letting the process die
    /// while that thread is still inside the DLL can deadlock shutdown and leave
    /// the app running in the background.
    pub fn shutdown_acq(&self, timeout: std::time::Duration) {
        let handle = self.acq.lock().ok().and_then(|mut slot| slot.take());
        if let Some(h) = handle {
            h.shutdown(timeout);
        }
    }

    pub fn set_acq(&self, handle: AcqHandle) {
        if let Ok(mut slot) = self.acq.lock() {
            *slot = Some(handle);
        }
    }

    /// Persist the current settings to disk (best effort).
    pub fn persist(&self) {
        if let Ok(settings) = self.settings.lock() {
            let _ = crate::config::save(&self.config_dir, &settings);
        }
    }
}
