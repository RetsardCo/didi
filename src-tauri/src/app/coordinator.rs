use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::errors::DiDiError;
use crate::presence::{ApplicationId, PresenceActivity, PresenceServiceHandle};
use crate::storage::{
    AppConfigV1, ConfigStore, ImportConflictStrategy, ImportPreview, ProfileV1, StorageError,
    StoreLoadStatus, CURRENT_SCHEMA_VERSION, MAX_IMPORT_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupNoticeKind {
    Normal,
    CreatedDefault,
    RecoveredFromBackup,
    ResetToDefaultAfterCorruption,
    UnsupportedFutureSchema,
    StorageLoadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupNoticeDto {
    pub kind: StartupNoticeKind,
    pub message: Option<String>,
    pub can_edit_config: bool,
    pub found_schema_version: Option<u32>,
    pub supported_schema_version: u32,
    pub quarantined_filename: Option<String>,
}

impl StartupNoticeDto {
    #[must_use]
    pub fn from_load_success(status: StoreLoadStatus) -> Self {
        match status {
            StoreLoadStatus::Normal => Self {
                kind: StartupNoticeKind::Normal,
                message: None,
                can_edit_config: true,
                found_schema_version: None,
                supported_schema_version: CURRENT_SCHEMA_VERSION,
                quarantined_filename: None,
            },
            StoreLoadStatus::CreatedDefault => Self {
                kind: StartupNoticeKind::CreatedDefault,
                message: None,
                can_edit_config: true,
                found_schema_version: None,
                supported_schema_version: CURRENT_SCHEMA_VERSION,
                quarantined_filename: None,
            },
            StoreLoadStatus::RecoveredFromBackup { quarantined_file } => Self {
                kind: StartupNoticeKind::RecoveredFromBackup,
                message: Some(
                    "Configuration was corrupted and safely recovered from your last known good backup."
                        .to_owned(),
                ),
                can_edit_config: true,
                found_schema_version: None,
                supported_schema_version: CURRENT_SCHEMA_VERSION,
                quarantined_filename: extract_filename_only(&quarantined_file),
            },
            StoreLoadStatus::ResetToDefaultAfterCorruption { quarantined_file } => Self {
                kind: StartupNoticeKind::ResetToDefaultAfterCorruption,
                message: Some(
                    "Configuration was corrupted and reset to safe defaults. A copy of the damaged file was quarantined."
                        .to_owned(),
                ),
                can_edit_config: true,
                found_schema_version: None,
                supported_schema_version: CURRENT_SCHEMA_VERSION,
                quarantined_filename: extract_filename_only(&quarantined_file),
            },
        }
    }

    #[must_use]
    pub fn from_load_error(err: &StorageError) -> Self {
        match err {
            StorageError::UnsupportedFutureSchema {
                found,
                supported_max,
            } => Self {
                kind: StartupNoticeKind::UnsupportedFutureSchema,
                message: Some(
                    "Configuration was created by a newer version of DiDi. Upgrade DiDi before editing settings."
                        .to_owned(),
                ),
                can_edit_config: false,
                found_schema_version: Some(*found),
                supported_schema_version: *supported_max,
                quarantined_filename: None,
            },
            other => {
                let safe = DiDiError::Storage(StorageError::Io(std::io::Error::other(
                    other.to_string(),
                )))
                .user_safe_message();
                Self {
                    kind: StartupNoticeKind::StorageLoadFailed,
                    message: Some(safe),
                    can_edit_config: false,
                    found_schema_version: None,
                    supported_schema_version: CURRENT_SCHEMA_VERSION,
                    quarantined_filename: None,
                }
            }
        }
    }

    #[must_use]
    pub fn blocked_edit_error(&self) -> DiDiError {
        match self.kind {
            StartupNoticeKind::UnsupportedFutureSchema => {
                DiDiError::Storage(StorageError::UnsupportedFutureSchema {
                    found: self
                        .found_schema_version
                        .unwrap_or(CURRENT_SCHEMA_VERSION + 1),
                    supported_max: self.supported_schema_version,
                })
            }
            _ => DiDiError::Storage(StorageError::Io(std::io::Error::other(
                "startup configuration load failed",
            ))),
        }
    }
}

fn extract_filename_only(raw_path: &str) -> Option<String> {
    Path::new(raw_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn resolve_active_profile_presence(
    config: &AppConfigV1,
) -> Result<(Option<ApplicationId>, PresenceActivity), DiDiError> {
    let active_p = config.active_profile()?;
    let effective_app_id =
        active_p.effective_application_id(config.default_application_id.as_deref());
    let parsed_id = match effective_app_id {
        Some(raw) => {
            Some(ApplicationId::parse(raw).map_err(|err| DiDiError::Validation(err.into()))?)
        }
        None => None,
    };
    let activity = active_p.to_presence_activity(None);
    Ok((parsed_id, activity))
}

/// Resolves the effective Application ID and `PresenceActivity` from `config` and atomically
/// applies them to `PresenceService` if `presence_enabled` is true.
pub async fn sync_active_profile_to_service(
    config: &AppConfigV1,
    service: &PresenceServiceHandle,
) -> Result<(), DiDiError> {
    if !config.presence_enabled {
        return Ok(());
    }

    let (parsed_id, activity) = resolve_active_profile_presence(config)?;
    service.apply_profile(parsed_id, activity).await
}

/// Authoritative shared Pause/Resume path used by both the system tray menu and frontend
/// commands (`DEF-01`). Keeps `ConfigStore::presence_enabled` and `PresenceService` synchronized.
pub async fn set_presence_enabled_synced(
    store: &ConfigStore,
    service: &PresenceServiceHandle,
    startup_notice: Option<&StartupNoticeDto>,
    enabled: bool,
) -> Result<AppConfigV1, DiDiError> {
    if let Some(notice) = startup_notice {
        if !notice.can_edit_config {
            return Err(notice.blocked_edit_error());
        }
    }

    let updated_config = store.set_presence_enabled(enabled)?;

    if enabled {
        let (parsed_id, activity) = resolve_active_profile_presence(&updated_config)?;
        service
            .resume_with_profile(parsed_id, Some(activity))
            .await?;
    } else {
        service.pause().await?;
    }

    Ok(updated_config)
}

#[derive(Debug, Clone)]
struct PendingImport {
    token: String,
    profiles: Vec<ProfileV1>,
}

/// Holds a validated profile import bundle between native-dialog preview and user confirmation,
/// ensuring JavaScript never supplies or receives arbitrary filesystem paths (`DEF-12`).
#[derive(Default)]
pub struct PendingImportState {
    pending: Mutex<Option<PendingImport>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportPreviewSessionDto {
    pub import_token: String,
    pub preview: ImportPreview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportOutcomeDto {
    pub exported: bool,
}

pub fn validate_import_file_path(path: &Path) -> Result<u64, DiDiError> {
    let has_json_ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    if !has_json_ext {
        return Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "selected file must have a .json extension".to_owned(),
        )));
    }

    let meta =
        fs::symlink_metadata(path).map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    if !meta.file_type().is_file() {
        return Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "selected import target is not a regular file".to_owned(),
        )));
    }

    let size_bytes = meta.len();
    if size_bytes > MAX_IMPORT_BYTES as u64 {
        return Err(DiDiError::Storage(StorageError::ImportTooLarge {
            size_bytes: usize::try_from(size_bytes).unwrap_or(usize::MAX),
            max_bytes: MAX_IMPORT_BYTES,
        }));
    }

    Ok(size_bytes)
}

pub fn read_and_preview_import_file(
    store: &ConfigStore,
    pending_state: &PendingImportState,
    path: &Path,
) -> Result<ImportPreviewSessionDto, DiDiError> {
    validate_import_file_path(path)?;
    let bytes = fs::read(path).map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;

    let (preview, profiles) = store.preview_and_parse_import(&bytes)?;
    let token = uuid::Uuid::new_v4().to_string();

    {
        let mut guard = pending_state
            .pending
            .lock()
            .map_err(|_| DiDiError::Lifecycle("import state lock poisoned".to_owned()))?;
        *guard = Some(PendingImport {
            token: token.clone(),
            profiles,
        });
    }

    Ok(ImportPreviewSessionDto {
        import_token: token,
        preview,
    })
}

pub fn confirm_pending_import(
    store: &ConfigStore,
    pending_state: &PendingImportState,
    import_token: &str,
    strategy: ImportConflictStrategy,
) -> Result<AppConfigV1, DiDiError> {
    let staged = {
        let mut guard = pending_state
            .pending
            .lock()
            .map_err(|_| DiDiError::Lifecycle("import state lock poisoned".to_owned()))?;
        match guard.as_ref() {
            Some(pending) if pending.token == import_token => guard.take(),
            _ => None,
        }
    };

    let Some(pending) = staged else {
        return Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "no matching pending profile import found".to_owned(),
        )));
    };

    let updated = store.apply_validated_profiles(pending.profiles, strategy)?;
    Ok(updated)
}

pub fn validate_export_file_path(path: &Path) -> Result<(), DiDiError> {
    let has_json_ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
    if !has_json_ext {
        return Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(
            "export file must have a .json extension".to_owned(),
        )));
    }

    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_file() {
            return Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(
                "export destination is not a regular file".to_owned(),
            )));
        }
    }

    Ok(())
}

pub fn write_export_bundle_to_file(
    store: &ConfigStore,
    profile_ids: Option<Vec<String>>,
    path: &Path,
) -> Result<(), DiDiError> {
    validate_export_file_path(path)?;
    let bundle = store.export_bundle(profile_ids)?;
    let bytes = serde_json::to_vec_pretty(&bundle)
        .map_err(|err| DiDiError::Storage(StorageError::Json(err)))?;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    file.write_all(&bytes)
        .map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    file.flush()
        .map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    file.sync_all()
        .map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    Ok(())
}

/// Strictly allowlisted external documentation/portal targets for safe shell opening (`Section 6.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalDocsTarget {
    DeveloperPortal,
    ProjectRepository,
    IssueTracker,
}

impl ExternalDocsTarget {
    #[must_use]
    pub const fn url(self) -> &'static str {
        match self {
            Self::DeveloperPortal => "https://discord.com/developers/applications",
            Self::ProjectRepository => "https://github.com/RetsardCo/didi",
            Self::IssueTracker => "https://github.com/RetsardCo/didi/issues",
        }
    }
}

/// Resolves and ensures the fixed app-owned `%APPDATA%\DiDi\logs\` directory (`Section 6.4`).
pub fn resolve_logs_dir(config_dir: &Path) -> Result<PathBuf, DiDiError> {
    let logs_dir = config_dir.join("logs");
    fs::create_dir_all(&logs_dir).map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    let meta =
        fs::symlink_metadata(&logs_dir).map_err(|err| DiDiError::Storage(StorageError::Io(err)))?;
    if !meta.file_type().is_dir() {
        return Err(DiDiError::Lifecycle(
            "logs path is not a directory".to_owned(),
        ));
    }
    Ok(logs_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::MockDiscordAdapter;
    use crate::presence::{PresenceService, PresenceState};
    use crate::storage::ExportBundleV1;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn create_temp_dir(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!(
            "didi_test_coord_{label}_{}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[tokio::test]
    async fn def01_shared_pause_resume_syncs_config_and_service_and_persists_restart(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;
        use std::time::Duration;

        let dir = create_temp_dir("pause_resume")?;
        let store = ConfigStore::new(dir.clone());
        let (mut cfg, _) = store.load_or_init()?;
        cfg.default_application_id = Some("123456789012345678".to_owned());
        store.save(&cfg)?;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let initial_act = cfg.active_profile()?.to_presence_activity(None);
        let wait_limit = Duration::from_secs(3);
        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(initial_act),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        // Pause via shared coordinator (same path as tray & frontend command)
        let after_pause = set_presence_enabled_synced(&store, &handle, None, false).await?;
        assert!(!after_pause.presence_enabled);
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled")??;
        assert_eq!(rx.borrow().active_activity, None);

        let recorded_pause = calls.lock().map_err(|_| "lock poisoned")?.clone();
        let clear_idx = recorded_pause
            .iter()
            .position(|c| matches!(c, MockAdapterCall::ClearActivity))
            .ok_or("missing ClearActivity call on shared Pause")?;
        let disc_idx = recorded_pause
            .iter()
            .position(|c| matches!(c, MockAdapterCall::Disconnect))
            .ok_or("missing Disconnect call on shared Pause")?;
        assert_eq!(disc_idx, clear_idx + 1);

        // Reload from disk to verify restart preserves paused preference
        let (reloaded, _) = store.load_or_init()?;
        assert!(!reloaded.presence_enabled);

        // Resume via shared coordinator restores active profile & App ID
        let after_resume = set_presence_enabled_synced(&store, &handle, None, true).await?;
        assert!(after_resume.presence_enabled);
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published after resume")??;
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("Custom Rich Presence")
        );

        handle.shutdown().await?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn def11_startup_notice_surfaces_recovery_and_future_schema_without_full_paths() {
        let normal = StartupNoticeDto::from_load_success(StoreLoadStatus::Normal);
        assert_eq!(normal.kind, StartupNoticeKind::Normal);
        assert!(normal.can_edit_config);
        assert_eq!(normal.message, None);

        let recovered = StartupNoticeDto::from_load_success(StoreLoadStatus::RecoveredFromBackup {
            quarantined_file: r"C:\Users\Alice\AppData\Roaming\DiDi\config.json.corrupt.123"
                .to_owned(),
        });
        assert_eq!(recovered.kind, StartupNoticeKind::RecoveredFromBackup);
        assert!(recovered.can_edit_config);
        assert_eq!(
            recovered.quarantined_filename.as_deref(),
            Some("config.json.corrupt.123")
        );
        assert!(!recovered
            .quarantined_filename
            .as_deref()
            .unwrap_or("")
            .contains("Alice"));

        let reset =
            StartupNoticeDto::from_load_success(StoreLoadStatus::ResetToDefaultAfterCorruption {
                quarantined_file: r"C:\Users\Alice\AppData\Roaming\DiDi\config.json.corrupt.456"
                    .to_owned(),
            });
        assert_eq!(reset.kind, StartupNoticeKind::ResetToDefaultAfterCorruption);
        assert!(reset.can_edit_config);
        assert_eq!(
            reset.quarantined_filename.as_deref(),
            Some("config.json.corrupt.456")
        );

        let future = StartupNoticeDto::from_load_error(&StorageError::UnsupportedFutureSchema {
            found: 3,
            supported_max: 1,
        });
        assert_eq!(future.kind, StartupNoticeKind::UnsupportedFutureSchema);
        assert!(!future.can_edit_config);
        assert_eq!(future.found_schema_version, Some(3));

        let io_err = StartupNoticeDto::from_load_error(&StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            r"C:\Users\Alice\AppData\Roaming\DiDi\config.json",
        )));
        assert_eq!(io_err.kind, StartupNoticeKind::StorageLoadFailed);
        assert!(!io_err.can_edit_config);
        assert!(!io_err.message.unwrap_or_default().contains("Alice"));
    }

    #[tokio::test]
    async fn def11_blocks_presence_enable_when_startup_notice_disallows_editing(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = create_temp_dir("blocked_startup")?;
        let store = ConfigStore::new(dir.clone());
        let _ = store.load_or_init()?;
        let (service, handle) = PresenceService::with_startup_error(
            Box::new(MockDiscordAdapter::new()),
            "Future".to_owned(),
        );
        tokio::spawn(service.run());

        let notice = StartupNoticeDto::from_load_error(&StorageError::UnsupportedFutureSchema {
            found: 2,
            supported_max: 1,
        });
        let res = set_presence_enabled_synced(&store, &handle, Some(&notice), true).await;
        assert!(matches!(
            res,
            Err(DiDiError::Storage(StorageError::UnsupportedFutureSchema {
                found: 2,
                supported_max: 1
            }))
        ));
        assert_eq!(handle.status().state, PresenceState::Error);

        let io_notice = StartupNoticeDto::from_load_error(&StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            r"C:\Users\Alice\AppData\Roaming\DiDi\config.json",
        )));
        let io_res = set_presence_enabled_synced(&store, &handle, Some(&io_notice), true).await;
        match io_res {
            Err(err) => {
                let msg = err.user_safe_message();
                assert!(!msg.contains("newer version"));
                assert!(!msg.contains("Alice"));
                assert!(msg.contains("reading or saving configuration"));
            }
            Ok(_) => return Err("expected StorageLoadFailed error".into()),
        }

        handle.shutdown().await?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn def12_import_export_file_boundary_validates_extension_size_type_and_tokens(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = create_temp_dir("import_export")?;
        let store = ConfigStore::new(dir.clone());
        let _ = store.load_or_init()?;
        let pending = PendingImportState::default();

        // 1. Export bundle to .json and overwrite existing file
        let export_path = dir.join("bundle.json");
        write_export_bundle_to_file(&store, None, &export_path)?;
        assert!(export_path.exists());
        write_export_bundle_to_file(&store, None, &export_path)?;

        // 2. Wrong extension rejected
        let txt_path = dir.join("bundle.txt");
        fs::write(&txt_path, b"{}")?;
        assert!(read_and_preview_import_file(&store, &pending, &txt_path).is_err());

        // 3. Missing file rejected
        let missing_path = dir.join("does_not_exist.json");
        assert!(read_and_preview_import_file(&store, &pending, &missing_path).is_err());

        // 4. Directory with .json name rejected
        let dir_json = dir.join("folder.json");
        fs::create_dir_all(&dir_json)?;
        assert!(read_and_preview_import_file(&store, &pending, &dir_json).is_err());

        // 5. Oversized file (> 256 KiB) rejected before reading
        let huge_path = dir.join("huge.json");
        fs::write(&huge_path, vec![b' '; MAX_IMPORT_BYTES + 1])?;
        assert!(matches!(
            read_and_preview_import_file(&store, &pending, &huge_path),
            Err(DiDiError::Storage(StorageError::ImportTooLarge { .. }))
        ));

        // 6. Invalid JSON rejected
        let bad_json_path = dir.join("bad.json");
        fs::write(&bad_json_path, b"{not valid json")?;
        assert!(matches!(
            read_and_preview_import_file(&store, &pending, &bad_json_path),
            Err(DiDiError::Storage(StorageError::ImportSchemaInvalid(_)))
        ));

        // 7. Valid two-step preview -> confirm import with token check
        let mut new_p = ProfileV1::default_profile();
        new_p.id = "imported_one".to_owned();
        new_p.name = "Imported One".to_owned();
        let valid_bundle = ExportBundleV1::from_profiles(&[new_p], 1700000000);
        fs::write(&export_path, serde_json::to_vec_pretty(&valid_bundle)?)?;

        let preview_session = read_and_preview_import_file(&store, &pending, &export_path)?;
        assert_eq!(preview_session.preview.new_profiles.len(), 1);

        // Wrong token rejected without consuming valid token
        assert!(confirm_pending_import(
            &store,
            &pending,
            "wrong-token",
            ImportConflictStrategy::KeepBoth
        )
        .is_err());

        let updated = confirm_pending_import(
            &store,
            &pending,
            &preview_session.import_token,
            ImportConflictStrategy::KeepBoth,
        )?;
        assert!(updated.find_profile("imported_one").is_some());

        // Reusing already-consumed token rejected
        assert!(confirm_pending_import(
            &store,
            &pending,
            &preview_session.import_token,
            ImportConflictStrategy::KeepBoth
        )
        .is_err());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn allowlisted_external_urls_and_logs_dir_are_strict() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            ExternalDocsTarget::DeveloperPortal.url(),
            "https://discord.com/developers/applications"
        );
        assert_eq!(
            ExternalDocsTarget::ProjectRepository.url(),
            "https://github.com/RetsardCo/didi"
        );
        assert_eq!(
            ExternalDocsTarget::IssueTracker.url(),
            "https://github.com/RetsardCo/didi/issues"
        );

        let dir = create_temp_dir("logs_dir")?;
        let logs = resolve_logs_dir(&dir)?;
        assert!(logs.is_dir());
        assert_eq!(logs, dir.join("logs"));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }
}
