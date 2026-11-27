use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, watch};

use crate::app::lifecycle::SETTINGS_WINDOW_LABEL;
use crate::presence::{PresenceServiceHandle, PresenceServiceStatus};

pub const PRESENCE_STATUS_EVENT: &str = "presence://status";
pub const STARTUP_PREFERENCE_EVENT: &str = "config://autostart-changed";

/// Narrow, non-sensitive event emitted to the open `"settings"` window whenever
/// the tray updates `launch_at_login` (`FINDING-03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutostartPreferenceChangedEvent {
    pub launch_at_login: bool,
}

pub fn emit_autostart_preference_changed<F>(launch_at_login: bool, mut emit_fn: F)
where
    F: FnMut(&str, AutostartPreferenceChangedEvent),
{
    emit_fn(
        STARTUP_PREFERENCE_EVENT,
        AutostartPreferenceChangedEvent { launch_at_login },
    );
}

pub fn notify_settings_autostart_changed(app: &AppHandle, launch_at_login: bool) {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        emit_autostart_preference_changed(launch_at_login, |event_name, payload| {
            let _ = window.emit(event_name, payload);
        });
    }
}

/// Spawns an event-driven status bridge that forwards `PresenceServiceStatus` updates
/// to the `"settings"` WebView window only while that window exists.
///
/// Terminates immediately when `stop_rx` fires (triggered on `WindowEvent::Destroyed`)
/// or when the service `watch` channel closes, with zero background polling.
pub fn spawn_presence_status_bridge(
    app: AppHandle,
    service: PresenceServiceHandle,
    stop_rx: oneshot::Receiver<()>,
) {
    let status_rx = service.subscribe();
    tauri::async_runtime::spawn(async move {
        forward_status_updates(status_rx, stop_rx, move |status| {
            if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
                let _ = window.emit(PRESENCE_STATUS_EVENT, status);
                true
            } else {
                false
            }
        })
        .await;
    });
}

/// Core event-driven forwarding loop, extracted for deterministic unit testing without a WebView.
pub async fn forward_status_updates<F>(
    mut status_rx: watch::Receiver<PresenceServiceStatus>,
    mut stop_rx: oneshot::Receiver<()>,
    mut emit_fn: F,
) where
    F: FnMut(PresenceServiceStatus) -> bool,
{
    loop {
        tokio::select! {
            _ = &mut stop_rx => {
                break;
            }
            changed = status_rx.changed() => {
                if changed.is_err() {
                    break;
                }
                let snapshot = status_rx.borrow().clone();
                if !emit_fn(snapshot) {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{ApplicationId, PresenceState};
    use std::sync::{Arc, Mutex};

    fn sample_status(state: PresenceState, masked_app_id: Option<&str>) -> PresenceServiceStatus {
        PresenceServiceStatus {
            state,
            status_label: state.label(),
            masked_app_id: masked_app_id.map(str::to_owned),
            is_connected: state == PresenceState::Published,
            active_activity: None,
            pending_activity: None,
            pipe_slot: None,
            protocol_version: None,
            consecutive_failures: 0,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn bridge_forwards_updates_without_polling_and_stops_on_window_destroy(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let app_id = ApplicationId::parse("123456789012345678")?;
        let (tx, rx) = watch::channel(sample_status(
            PresenceState::WaitingForDiscord,
            Some(&app_id.masked()),
        ));
        let (stop_tx, stop_rx) = oneshot::channel();
        let emitted = Arc::new(Mutex::new(Vec::new()));
        let emitted_clone = emitted.clone();

        let task = tokio::spawn(forward_status_updates(rx, stop_rx, move |status| {
            if let Ok(mut guard) = emitted_clone.lock() {
                guard.push(status);
            }
            true
        }));

        tx.send(sample_status(
            PresenceState::Connecting,
            Some(&app_id.masked()),
        ))?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        tx.send(sample_status(
            PresenceState::Published,
            Some(&app_id.masked()),
        ))?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Destroy window -> bridge task must exit cleanly
        let _ = stop_tx.send(());
        task.await?;

        // Further status updates after stop must not be emitted
        let _ = tx.send(sample_status(PresenceState::Disabled, None));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let list = emitted.lock().map_err(|_| "lock poisoned")?.clone();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].state, PresenceState::Connecting);
        assert_eq!(list[1].state, PresenceState::Published);

        // Verify serialized status never exposes unmasked Application ID
        let json = serde_json::to_string(&list[1])?;
        assert!(json.contains("****5678"));
        assert!(!json.contains("123456789012345678"));
        Ok(())
    }

    #[test]
    fn autostart_event_serializes_minimal_non_sensitive_payload(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut captured = Vec::new();
        emit_autostart_preference_changed(true, |name, payload| {
            captured.push((name.to_owned(), payload));
        });
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].0, STARTUP_PREFERENCE_EVENT);
        assert_eq!(
            captured[0].1,
            AutostartPreferenceChangedEvent {
                launch_at_login: true
            }
        );

        let json = serde_json::to_string(&captured[0].1)?;
        assert_eq!(json, r#"{"launch_at_login":true}"#);
        Ok(())
    }

    #[tokio::test]
    async fn bridge_exits_when_window_lookup_returns_false(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (tx, rx) = watch::channel(sample_status(PresenceState::Disabled, None));
        let (_stop_tx, stop_rx) = oneshot::channel();

        let task = tokio::spawn(forward_status_updates(rx, stop_rx, |_| false));
        tx.send(sample_status(PresenceState::WaitingForDiscord, None))?;
        task.await?;
        Ok(())
    }
}
