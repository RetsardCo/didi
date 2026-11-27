use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use crate::app::coordinator::{
    confirm_pending_import, read_and_preview_import_file, resolve_logs_dir,
    set_presence_enabled_synced, sync_active_profile_to_service, write_export_bundle_to_file,
    ExportOutcomeDto, ExternalDocsTarget, ImportPreviewSessionDto, PendingImportState,
    StartupNoticeDto,
};
use crate::app::lifecycle::{
    apply_window_layout_size, request_exit, AppLifecycleState, SETTINGS_WINDOW_LABEL,
};
use crate::app::startup::{sync_preferences_with_autostart, TauriAutostartBackend};
use crate::app::tray::sync_tray_autostart_checkmark;
use crate::errors::DiDiError;
use crate::presence::{PresenceServiceHandle, PresenceServiceStatus, PresenceState};
use crate::storage::{
    AppConfigV1, ConfigStore, ImportConflictStrategy, PreferencesPatch, ProfileV1, StorageError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScaffoldStatusDto {
    pub product_name: &'static str,
    pub expanded_name: &'static str,
    pub version: &'static str,
    pub stage: &'static str,
    pub presence_state: String,
    pub adapter_kind: &'static str,
    pub live_ipc_enabled: bool,
}

#[must_use]
pub fn build_scaffold_status(state_label: Option<&str>) -> ScaffoldStatusDto {
    ScaffoldStatusDto {
        product_name: "DiDi",
        expanded_name: "Discord Ditto",
        version: env!("CARGO_PKG_VERSION"),
        stage: "Phase 5 — Profile Data Model & Atomic Persistence",
        presence_state: state_label
            .unwrap_or(PresenceState::Disabled.label())
            .to_owned(),
        adapter_kind: "LiveDiscordIpcAdapter",
        live_ipc_enabled: true,
    }
}

fn ensure_config_editable(notice: &StartupNoticeDto) -> Result<(), String> {
    if notice.can_edit_config {
        Ok(())
    } else {
        Err(notice.blocked_edit_error().user_safe_message())
    }
}

/// Returns honest scaffold runtime status including the live Presence Service state.
#[tauri::command]
pub fn get_scaffold_status(service: State<'_, PresenceServiceHandle>) -> ScaffoldStatusDto {
    build_scaffold_status(Some(service.status().state.label()))
}

/// Returns the comprehensive observable status from the Presence Service Coordinator.
#[tauri::command]
pub fn get_presence_status(service: State<'_, PresenceServiceHandle>) -> PresenceServiceStatus {
    service.status()
}

/// Returns the startup configuration recovery/integrity notice captured during launch.
#[tauri::command]
pub fn get_startup_notice(notice: State<'_, StartupNoticeDto>) -> StartupNoticeDto {
    notice.inner().clone()
}

/// Triggers immediate retry from an Error or Waiting banner.
#[tauri::command]
pub async fn retry_presence(
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
) -> Result<(), String> {
    ensure_config_editable(notice.inner())?;
    service
        .retry_now()
        .await
        .map_err(|err| err.user_safe_message())
}

/// Explicit application Exit: closes window, disconnects IPC with <= 2.2s deadline, terminates.
#[tauri::command]
pub fn exit_app(app: AppHandle) {
    request_exit(&app);
}

/// Returns the full active configuration snapshot.
#[tauri::command]
pub fn get_app_config(store: State<'_, ConfigStore>) -> Result<AppConfigV1, String> {
    store
        .get_config()
        .map_err(|e| DiDiError::from(e).user_safe_message())
}

/// Saves or updates a profile in configuration, syncing atomically with Presence Service if active.
#[tauri::command]
pub async fn save_profile(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
    profile: ProfileV1,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;
    let normalized_id = profile.id.trim().to_owned();
    let updated_config = store
        .save_profile(profile)
        .map_err(|e| DiDiError::from(e).user_safe_message())?;

    if updated_config.active_profile_id == normalized_id {
        sync_active_profile_to_service(&updated_config, service.inner())
            .await
            .map_err(|e| e.user_safe_message())?;
    }

    Ok(updated_config)
}

/// Deletes a non-last profile. Only updates Presence Service if the active profile was deleted.
#[tauri::command]
pub async fn delete_profile(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
    profile_id: String,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;
    let was_active = {
        let cfg = store
            .get_config()
            .map_err(|e| DiDiError::from(e).user_safe_message())?;
        cfg.active_profile_id == profile_id
    };

    let updated_config = store
        .delete_profile(&profile_id)
        .map_err(|e| DiDiError::from(e).user_safe_message())?;

    if was_active {
        sync_active_profile_to_service(&updated_config, service.inner())
            .await
            .map_err(|e| e.user_safe_message())?;
    }

    Ok(updated_config)
}

/// Sets the active profile ID, atomically updating Presence Service with its App ID and activity.
#[tauri::command]
pub async fn set_active_profile(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
    profile_id: String,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;
    let updated_config = store
        .set_active_profile(&profile_id)
        .map_err(|e| DiDiError::from(e).user_safe_message())?;

    sync_active_profile_to_service(&updated_config, service.inner())
        .await
        .map_err(|e| e.user_safe_message())?;

    Ok(updated_config)
}

/// Updates the default Application ID in configuration and syncs the active profile atomically.
#[tauri::command]
pub async fn set_default_application_id(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
    raw_app_id: Option<String>,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;
    let updated_config = store
        .set_default_application_id(raw_app_id)
        .map_err(|e| DiDiError::from(e).user_safe_message())?;

    sync_active_profile_to_service(&updated_config, service.inner())
        .await
        .map_err(|e| e.user_safe_message())?;

    Ok(updated_config)
}

/// Toggles presence enabled state in configuration and coordinator.
#[tauri::command]
pub async fn set_presence_enabled(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    notice: State<'_, StartupNoticeDto>,
    enabled: bool,
) -> Result<AppConfigV1, String> {
    set_presence_enabled_synced(
        store.inner(),
        service.inner(),
        Some(notice.inner()),
        enabled,
    )
    .await
    .map_err(|e| e.user_safe_message())
}

/// Updates application preferences (`launch_at_login`, `start_minimized`, `close_to_tray`, `ui_layout`)
/// atomically while preserving unspecified fields and synchronizing autostart, lifecycle, and window size.
#[tauri::command]
pub fn update_preferences(
    app: AppHandle,
    store: State<'_, ConfigStore>,
    lifecycle: State<'_, AppLifecycleState>,
    notice: State<'_, StartupNoticeDto>,
    patch: PreferencesPatch,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;

    let launch_changed = patch.launch_at_login.is_some();
    let layout_changed = patch.ui_layout;

    let backend = TauriAutostartBackend(&app);
    let updated = sync_preferences_with_autostart(store.inner(), &backend, patch)
        .map_err(|e| e.user_safe_message())?;

    if launch_changed {
        sync_tray_autostart_checkmark(&app, updated.launch_at_login);
    }

    lifecycle.set_close_to_tray(updated.close_to_tray);
    if let Some(layout) = layout_changed {
        apply_window_layout_size(&app, layout).map_err(|e| e.user_safe_message())?;
    }

    Ok(updated)
}

/// Opens a native file dialog (in Rust) for selecting a `.json` profile bundle, validates it,
/// and returns a staged preview with an opaque token (never accepting or exposing OS paths to JS).
#[tauri::command]
pub async fn pick_and_preview_profile_import(
    app: AppHandle,
    store: State<'_, ConfigStore>,
    pending: State<'_, PendingImportState>,
    notice: State<'_, StartupNoticeDto>,
) -> Result<Option<ImportPreviewSessionDto>, String> {
    ensure_config_editable(notice.inner())?;

    let (tx, rx) = oneshot::channel();
    let mut builder = app
        .dialog()
        .file()
        .set_title("Import DiDi Profiles")
        .add_filter("DiDi Profile Bundle", &["json"]);
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        builder = builder.set_parent(&window);
    }
    builder.pick_file(move |picked| {
        let _ = tx.send(picked);
    });

    let Some(file_path) = rx.await.map_err(|_| {
        DiDiError::Lifecycle("file picker cancelled".to_owned()).user_safe_message()
    })?
    else {
        return Ok(None);
    };

    let path = file_path.into_path().map_err(|_| {
        DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "invalid file path".to_owned(),
        ))
        .user_safe_message()
    })?;

    read_and_preview_import_file(store.inner(), pending.inner(), &path)
        .map(Some)
        .map_err(|e| e.user_safe_message())
}

/// Applies a previously previewed import bundle identified by its opaque token (`DEF-12` / `DEF-14`).
#[tauri::command]
pub async fn confirm_profile_import(
    store: State<'_, ConfigStore>,
    service: State<'_, PresenceServiceHandle>,
    pending: State<'_, PendingImportState>,
    notice: State<'_, StartupNoticeDto>,
    import_token: String,
    strategy: ImportConflictStrategy,
) -> Result<AppConfigV1, String> {
    ensure_config_editable(notice.inner())?;

    let updated_config =
        confirm_pending_import(store.inner(), pending.inner(), &import_token, strategy)
            .map_err(|e| e.user_safe_message())?;

    sync_active_profile_to_service(&updated_config, service.inner())
        .await
        .map_err(|e| e.user_safe_message())?;

    Ok(updated_config)
}

/// Exports credential-free profiles to a `.json` file selected via a native Rust save dialog.
#[tauri::command]
pub async fn export_profiles_to_file(
    app: AppHandle,
    store: State<'_, ConfigStore>,
    profile_ids: Option<Vec<String>>,
) -> Result<ExportOutcomeDto, String> {
    // Validate requested profile IDs before opening the native save dialog
    let _ = store
        .export_bundle(profile_ids.clone())
        .map_err(|e| DiDiError::from(e).user_safe_message())?;

    let (tx, rx) = oneshot::channel();
    let mut builder = app
        .dialog()
        .file()
        .set_title("Export DiDi Profiles")
        .set_file_name("didi-profiles.json")
        .add_filter("DiDi Profile Bundle", &["json"]);
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
        builder = builder.set_parent(&window);
    }
    builder.save_file(move |picked| {
        let _ = tx.send(picked);
    });

    let Some(file_path) = rx.await.map_err(|_| {
        DiDiError::Lifecycle("save dialog cancelled".to_owned()).user_safe_message()
    })?
    else {
        return Ok(ExportOutcomeDto { exported: false });
    };

    let path = file_path.into_path().map_err(|_| {
        DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "invalid export path".to_owned(),
        ))
        .user_safe_message()
    })?;

    write_export_bundle_to_file(store.inner(), profile_ids, &path)
        .map_err(|e| e.user_safe_message())?;

    Ok(ExportOutcomeDto { exported: true })
}

/// Opens `%APPDATA%\DiDi\logs\` in Windows Explorer (fixed path resolved in Rust).
#[tauri::command]
pub fn open_logs_folder(store: State<'_, ConfigStore>) -> Result<(), String> {
    let logs_dir = resolve_logs_dir(store.config_dir()).map_err(|e| e.user_safe_message())?;
    tauri_plugin_opener::open_path(&logs_dir, None::<&str>)
        .map_err(|err| DiDiError::Lifecycle(err.to_string()).user_safe_message())
}

/// Opens a strictly allowlisted external documentation/portal URL in the default browser.
#[tauri::command]
pub fn open_external_docs_url(target: ExternalDocsTarget) -> Result<(), String> {
    tauri_plugin_opener::open_url(target.url(), None::<&str>)
        .map_err(|err| DiDiError::Lifecycle(err.to_string()).user_safe_message())
}

#[cfg(test)]
mod tests {
    use super::build_scaffold_status;

    #[test]
    fn scaffold_status_reports_phase_5_and_live_ipc_ready() {
        let status = build_scaffold_status(Some("Waiting for Discord"));
        assert_eq!(status.product_name, "DiDi");
        assert_eq!(status.expanded_name, "Discord Ditto");
        assert_eq!(
            status.stage,
            "Phase 5 — Profile Data Model & Atomic Persistence"
        );
        assert_eq!(status.presence_state, "Waiting for Discord");
        assert_eq!(status.adapter_kind, "LiveDiscordIpcAdapter");
        assert!(status.live_ipc_enabled);
    }
}
