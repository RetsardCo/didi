use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::app::coordinator::{set_presence_enabled_synced, StartupNoticeDto};
use crate::app::events::notify_settings_autostart_changed;
use crate::app::lifecycle::{open_or_focus_settings_window, request_exit};
use crate::app::startup::{
    is_autostart_enabled, sync_autostart_setting, AutostartBackend, TauriAutostartBackend,
};
use crate::errors::DiDiError;
use crate::presence::{PresenceServiceHandle, PresenceServiceStatus, PresenceState};
use crate::storage::ConfigStore;

pub const TRAY_ID: &str = "didi-tray";
pub const MENU_STATUS_ID: &str = "status";
pub const MENU_OPEN_ID: &str = "open";
pub const MENU_PAUSE_ID: &str = "pause";
pub const MENU_AUTOSTART_ID: &str = "autostart";
pub const MENU_EXIT_ID: &str = "exit";

/// Managed handle allowing `update_preferences` to keep the tray autostart checkmark immediately in sync (`DEF-02`).
#[derive(Clone)]
pub struct TrayAutostartHandle(pub CheckMenuItem<tauri::Wry>);

/// Executes the tray autostart toggle synchronously against `ConfigStore` + `AutostartBackend`,
/// returns the final persisted boolean (reverting on failure), and notifies open settings UI
/// strictly on persisted changes (`FINDING-03`).
pub fn handle_tray_autostart_toggle<F>(
    store: &ConfigStore,
    backend: &dyn AutostartBackend,
    fallback_enabled: bool,
    mut on_persisted: F,
) -> bool
where
    F: FnMut(bool),
{
    let current = store
        .get_config()
        .map(|cfg| cfg.launch_at_login)
        .unwrap_or(fallback_enabled);
    let target = !current;
    match sync_autostart_setting(store, backend, target) {
        Ok(updated) => {
            on_persisted(updated.launch_at_login);
            updated.launch_at_login
        }
        Err(_) => store
            .get_config()
            .map(|cfg| cfg.launch_at_login)
            .unwrap_or(current),
    }
}

pub fn sync_tray_autostart_checkmark(app: &AppHandle, checked: bool) {
    if let Some(item) = app.try_state::<TrayAutostartHandle>() {
        let _ = item.0.set_checked(checked);
    }
}

/// Formats tray tooltip and disabled status menu item text, surfacing unconfirmed activity
/// clearing when paused with a non-empty `last_error`.
#[must_use]
pub fn format_tray_status_strings(status: &PresenceServiceStatus) -> (String, String) {
    let label = status.state.label();
    if status.state == PresenceState::Disabled && status.last_error.is_some() {
        (
            format!("DiDi — {label} (Clear Unconfirmed)"),
            format!("Status: {label} (Clear Unconfirmed)"),
        )
    } else {
        (format!("DiDi — {label}"), format!("Status: {label}"))
    }
}

pub fn setup_tray(app: &AppHandle) -> Result<(), DiDiError> {
    let can_edit = app
        .try_state::<StartupNoticeDto>()
        .map(|n| n.can_edit_config)
        .unwrap_or(true);

    let status_item =
        MenuItem::with_id(app, MENU_STATUS_ID, "Status: Starting", false, None::<&str>)
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let open_item = MenuItem::with_id(app, MENU_OPEN_ID, "Open DiDi", true, None::<&str>)
        .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let pause_item =
        MenuItem::with_id(app, MENU_PAUSE_ID, "Pause Presence", can_edit, None::<&str>)
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let autostart_checked = app
        .try_state::<ConfigStore>()
        .and_then(|s| s.get_config().ok())
        .map(|cfg| cfg.launch_at_login)
        .unwrap_or_else(|| is_autostart_enabled(app).unwrap_or(false));
    let autostart_item = CheckMenuItem::with_id(
        app,
        MENU_AUTOSTART_ID,
        "Launch at Windows Startup",
        can_edit,
        autostart_checked,
        None::<&str>,
    )
    .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    app.manage(TrayAutostartHandle(autostart_item.clone()));

    let exit_item = MenuItem::with_id(app, MENU_EXIT_ID, "Exit", true, None::<&str>)
        .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &open_item,
            &pause_item,
            &autostart_item,
            &exit_item,
        ],
    )
    .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("DiDi — Discord Ditto")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event({
            let autostart_item_menu = autostart_item.clone();
            move |app_handle, event| match event.id().as_ref() {
                MENU_OPEN_ID => {
                    let _ = open_or_focus_settings_window(app_handle);
                }
                MENU_PAUSE_ID => {
                    if app_handle.try_state::<ConfigStore>().is_some() {
                        if let Some(service) = app_handle.try_state::<PresenceServiceHandle>() {
                            let app_clone = app_handle.clone();
                            let service = service.inner().clone();
                            let target_enabled = service.status().state == PresenceState::Disabled;
                            tauri::async_runtime::spawn(async move {
                                let store = app_clone.state::<ConfigStore>();
                                let notice = app_clone.try_state::<StartupNoticeDto>();
                                let _ = set_presence_enabled_synced(
                                    store.inner(),
                                    &service,
                                    notice.as_deref(),
                                    target_enabled,
                                )
                                .await;
                            });
                        }
                    }
                }
                MENU_AUTOSTART_ID => {
                    if let Some(store) = app_handle.try_state::<ConfigStore>() {
                        let fallback = is_autostart_enabled(app_handle).unwrap_or(false);
                        let backend = TauriAutostartBackend(app_handle);
                        let final_checked = handle_tray_autostart_toggle(
                            store.inner(),
                            &backend,
                            fallback,
                            |enabled| {
                                notify_settings_autostart_changed(app_handle, enabled);
                            },
                        );
                        let _ = autostart_item_menu.set_checked(final_checked);
                    }
                }
                MENU_EXIT_ID => {
                    request_exit(app_handle);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = open_or_focus_settings_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let tray = builder
        .build(app)
        .map_err(|err| DiDiError::Lifecycle(err.to_string()))?;

    // Spawn dynamic tray status updater task observing the presence coordinator
    if let Some(service_state) = app.try_state::<PresenceServiceHandle>() {
        let app_handle = app.clone();
        let service = service_state.inner().clone();
        let status_item_clone = status_item.clone();
        let pause_item_clone = pause_item.clone();
        let autostart_item_clone = autostart_item.clone();

        tauri::async_runtime::spawn(async move {
            let mut status_rx = service.subscribe();
            loop {
                let status = status_rx.borrow().clone();
                let (tooltip_text, status_text) = format_tray_status_strings(&status);

                let _ = tray.set_tooltip(Some(tooltip_text));
                let _ = status_item_clone.set_text(status_text);

                let pause_text = if status.state == PresenceState::Disabled {
                    "Resume Presence"
                } else {
                    "Pause Presence"
                };
                let _ = pause_item_clone.set_text(pause_text);

                let autostart_on = app_handle
                    .try_state::<ConfigStore>()
                    .and_then(|s| s.get_config().ok())
                    .map(|cfg| cfg.launch_at_login)
                    .unwrap_or_else(|| is_autostart_enabled(&app_handle).unwrap_or(false));
                let _ = autostart_item_clone.set_checked(autostart_on);

                if status_rx.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestAutostartBackend {
        enabled: Cell<bool>,
        fail_set: Cell<bool>,
    }

    impl TestAutostartBackend {
        fn new(enabled: bool) -> Self {
            Self {
                enabled: Cell::new(enabled),
                fail_set: Cell::new(false),
            }
        }
    }

    impl AutostartBackend for TestAutostartBackend {
        fn is_enabled(&self) -> Result<bool, DiDiError> {
            Ok(self.enabled.get())
        }

        fn set_enabled(&self, enable: bool) -> Result<(), DiDiError> {
            if self.fail_set.get() {
                return Err(DiDiError::Startup("registry failure".to_owned()));
            }
            self.enabled.set(enable);
            Ok(())
        }
    }

    fn temp_store_dir(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!(
            "didi_test_tray_{label}_{}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn tray_autostart_toggle_enables_disables_and_emits_only_on_success(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = temp_store_dir("toggle")?;
        let store = ConfigStore::new(dir.clone());
        let (cfg, _) = store.load_or_init()?;
        assert!(!cfg.launch_at_login);

        let backend = TestAutostartBackend::new(false);
        let mut emitted = Vec::new();

        // 1. Tray enables startup
        let checked_1 = handle_tray_autostart_toggle(&store, &backend, false, |v| emitted.push(v));
        assert!(checked_1);
        assert!(backend.is_enabled()?);
        assert!(store.get_config()?.launch_at_login);
        assert_eq!(emitted, vec![true]);

        // 2. Tray disables startup
        let checked_2 = handle_tray_autostart_toggle(&store, &backend, true, |v| emitted.push(v));
        assert!(!checked_2);
        assert!(!backend.is_enabled()?);
        assert!(!store.get_config()?.launch_at_login);
        assert_eq!(emitted, vec![true, false]);

        // 3. Registry failure reverts checkmark and emits nothing
        backend.fail_set.set(true);
        let checked_3 = handle_tray_autostart_toggle(&store, &backend, false, |v| emitted.push(v));
        assert!(!checked_3);
        assert!(!store.get_config()?.launch_at_login);
        assert_eq!(emitted, vec![true, false]);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn tray_autostart_toggle_does_not_emit_when_config_blocked_by_future_schema(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = temp_store_dir("future_schema")?;
        let store = ConfigStore::new(dir.clone());
        fs::write(dir.join("config.json"), r#"{"schema_version": 99}"#)?;

        let backend = TestAutostartBackend::new(false);
        let mut emitted = Vec::new();

        let checked = handle_tray_autostart_toggle(&store, &backend, false, |v| emitted.push(v));
        assert!(!checked);
        assert!(!backend.is_enabled()?);
        assert!(emitted.is_empty());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn tray_status_formatting_marks_disabled_clear_unconfirmed() {
        let base_status = |state: PresenceState, last_error: Option<&str>| PresenceServiceStatus {
            state,
            status_label: state.label(),
            masked_app_id: None,
            is_connected: false,
            active_activity: None,
            pending_activity: None,
            pipe_slot: None,
            protocol_version: None,
            consecutive_failures: 0,
            last_error: last_error.map(str::to_owned),
        };

        assert_eq!(
            format_tray_status_strings(&base_status(
                PresenceState::Disabled,
                Some("Discord did not respond in time to confirm clearing your activity.")
            )),
            (
                "DiDi — Disabled (Clear Unconfirmed)".to_owned(),
                "Status: Disabled (Clear Unconfirmed)".to_owned(),
            )
        );
        assert_eq!(
            format_tray_status_strings(&base_status(PresenceState::Disabled, None)),
            ("DiDi — Disabled".to_owned(), "Status: Disabled".to_owned(),)
        );
        assert_eq!(
            format_tray_status_strings(&base_status(PresenceState::Published, None)),
            (
                "DiDi — Published".to_owned(),
                "Status: Published".to_owned(),
            )
        );
    }
}
