use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, LogicalSize, Manager, Size, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokio::sync::oneshot;

use crate::app::events::spawn_presence_status_bridge;
use crate::errors::DiDiError;
use crate::presence::PresenceServiceHandle;
use crate::storage::{ConfigStore, UiLayout};

pub const SETTINGS_WINDOW_LABEL: &str = "settings";
pub const SETTINGS_DEFAULT_WIDTH: f64 = 480.0;
pub const SETTINGS_DEFAULT_HEIGHT: f64 = 600.0;
pub const SETTINGS_EXPANDED_WIDTH: f64 = 760.0;
pub const SETTINGS_EXPANDED_HEIGHT: f64 = 620.0;
const SETTINGS_MIN_WIDTH: f64 = 400.0;
const SETTINGS_MIN_HEIGHT: f64 = 480.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCloseAction {
    /// Allow WebView2 window destruction while preserving the background tray host (`close_to_tray = true`).
    AllowDestroyToTray,
    /// Allow window destruction as part of an in-progress explicit exit.
    AllowDestroyForExit,
    /// Prevent raw close and run bounded clean shutdown + process exit (`close_to_tray = false`).
    TriggerAppExit,
}

#[must_use]
pub const fn resolve_window_close_action(
    is_exiting: bool,
    close_to_tray: bool,
) -> WindowCloseAction {
    if is_exiting {
        WindowCloseAction::AllowDestroyForExit
    } else if close_to_tray {
        WindowCloseAction::AllowDestroyToTray
    } else {
        WindowCloseAction::TriggerAppExit
    }
}

#[must_use]
pub const fn window_dimensions_for_layout(layout: UiLayout) -> (f64, f64) {
    match layout {
        UiLayout::Compact => (SETTINGS_DEFAULT_WIDTH, SETTINGS_DEFAULT_HEIGHT),
        UiLayout::Expanded => (SETTINGS_EXPANDED_WIDTH, SETTINGS_EXPANDED_HEIGHT),
    }
}

/// Shared lifecycle coordination state.
///
/// Tracks explicit Exit and `close_to_tray` policy so closing the settings window either destroys
/// the WebView2 instance while keeping the tray host alive (D-019) or executes bounded shutdown.
pub struct AppLifecycleState {
    exiting: AtomicBool,
    close_to_tray: AtomicBool,
}

impl Default for AppLifecycleState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLifecycleState {
    #[must_use]
    pub fn new() -> Self {
        Self::with_close_to_tray(true)
    }

    #[must_use]
    pub fn with_close_to_tray(close_to_tray: bool) -> Self {
        Self {
            exiting: AtomicBool::new(false),
            close_to_tray: AtomicBool::new(close_to_tray),
        }
    }

    /// Atomically transitions into exiting state. Returns `true` only for the first caller,
    /// preventing duplicate shutdown sequences (`DEF-03` / `DEF-09`).
    pub fn begin_exit(&self) -> bool {
        !self.exiting.swap(true, Ordering::SeqCst)
    }

    #[must_use]
    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn close_to_tray(&self) -> bool {
        self.close_to_tray.load(Ordering::SeqCst)
    }

    pub fn set_close_to_tray(&self, enabled: bool) {
        self.close_to_tray.store(enabled, Ordering::SeqCst);
    }
}

/// Creates the on-demand settings `WebviewWindow` if destroyed, or focuses it if already open.
pub fn open_or_focus_settings_window(app: &AppHandle) -> Result<(), DiDiError> {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        window
            .unminimize()
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;
        window
            .show()
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;
        window
            .set_focus()
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;
        return Ok(());
    }

    let layout = app
        .try_state::<ConfigStore>()
        .and_then(|store| store.get_config().ok())
        .map(|cfg| cfg.ui_layout)
        .unwrap_or(UiLayout::Compact);
    let (width, height) = window_dimensions_for_layout(layout);

    let window = WebviewWindowBuilder::new(
        app,
        SETTINGS_WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("DiDi — Discord Ditto")
    .inner_size(width, height)
    .min_inner_size(SETTINGS_MIN_WIDTH, SETTINGS_MIN_HEIGHT)
    .resizable(true)
    .center()
    .build()
    .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let stop_tx_cell = Arc::new(Mutex::new(Some(stop_tx)));
    let app_for_event = app.clone();

    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { api, .. } => {
            let lifecycle = app_for_event.state::<AppLifecycleState>();
            let action =
                resolve_window_close_action(lifecycle.is_exiting(), lifecycle.close_to_tray());
            if action == WindowCloseAction::TriggerAppExit {
                api.prevent_close();
                request_exit(&app_for_event);
            }
        }
        WindowEvent::Destroyed => {
            if let Ok(mut guard) = stop_tx_cell.lock() {
                if let Some(tx) = guard.take() {
                    let _ = tx.send(());
                }
            }
        }
        _ => {}
    });

    if let Some(service) = app.try_state::<PresenceServiceHandle>() {
        spawn_presence_status_bridge(app.clone(), service.inner().clone(), stop_rx);
    }

    Ok(())
}

/// Resizes the open settings `WebviewWindow` when `ui_layout` changes in preferences.
pub fn apply_window_layout_size(app: &AppHandle, layout: UiLayout) -> Result<(), DiDiError> {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        let (width, height) = window_dimensions_for_layout(layout);
        window
            .set_size(Size::Logical(LogicalSize::new(width, height)))
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;
    }
    Ok(())
}

/// Deterministic explicit Exit: marks lifecycle exiting once, immediately hides any open settings
/// window to give instantaneous feedback, awaits clean presence service teardown (within <= 2.2s),
/// closes the window, and terminates the process.
pub fn request_exit(app: &AppHandle) {
    let lifecycle = app.state::<AppLifecycleState>();
    if !lifecycle.begin_exit() {
        return;
    }

    // Hide window immediately to eliminate visible UI delay while service disconnects cleanly
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        let _ = window.hide();
    }

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(service) = app_handle.try_state::<PresenceServiceHandle>() {
            let _ = tokio::time::timeout(Duration::from_millis(2200), service.shutdown()).await;
        }
        if let Some(window) = app_handle.get_webview_window(SETTINGS_WINDOW_LABEL) {
            let _ = window.close();
        }
        app_handle.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_state_defaults_to_running_and_prevents_duplicate_exit() {
        let state = AppLifecycleState::new();
        assert!(!state.is_exiting());
        assert!(state.close_to_tray());
        assert!(state.begin_exit());
        assert!(state.is_exiting());
        assert!(!state.begin_exit());
    }

    #[test]
    fn window_close_action_respects_close_to_tray_preference_and_exit_state() {
        assert_eq!(
            resolve_window_close_action(false, true),
            WindowCloseAction::AllowDestroyToTray
        );
        assert_eq!(
            resolve_window_close_action(false, false),
            WindowCloseAction::TriggerAppExit
        );
        assert_eq!(
            resolve_window_close_action(true, false),
            WindowCloseAction::AllowDestroyForExit
        );
        assert_eq!(
            resolve_window_close_action(true, true),
            WindowCloseAction::AllowDestroyForExit
        );
    }

    #[test]
    fn layout_dimensions_match_approved_specification() {
        assert_eq!(
            window_dimensions_for_layout(UiLayout::Compact),
            (480.0, 600.0)
        );
        assert_eq!(
            window_dimensions_for_layout(UiLayout::Expanded),
            (760.0, 620.0)
        );
    }
}
