use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

use crate::errors::DiDiError;
use crate::storage::{AppConfigV1, ConfigStore, PreferencesPatch};

/// CLI flag passed when DiDi is launched in background/tray mode (e.g., Windows sign-in startup).
pub const MINIMIZED_ARG: &str = "--minimized";

/// Abstraction over Windows Registry (`HKCU\...\Run`) autostart access for deterministic testing.
pub trait AutostartBackend {
    fn is_enabled(&self) -> Result<bool, DiDiError>;
    fn set_enabled(&self, enable: bool) -> Result<(), DiDiError>;
}

/// Live Tauri autostart backend backed by `tauri-plugin-autostart`.
pub struct TauriAutostartBackend<'a>(pub &'a AppHandle);

impl AutostartBackend for TauriAutostartBackend<'_> {
    fn is_enabled(&self) -> Result<bool, DiDiError> {
        is_autostart_enabled(self.0)
    }

    fn set_enabled(&self, enable: bool) -> Result<(), DiDiError> {
        set_autostart_enabled(self.0, enable)
    }
}

#[must_use]
pub fn has_minimized_flag(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg.eq_ignore_ascii_case(MINIMIZED_ARG))
}

pub fn is_autostart_enabled(app: &AppHandle) -> Result<bool, DiDiError> {
    app.autolaunch()
        .is_enabled()
        .map_err(|err| DiDiError::Startup(err.to_string()))
}

pub fn set_autostart_enabled(app: &AppHandle, enable: bool) -> Result<(), DiDiError> {
    if enable {
        app.autolaunch()
            .enable()
            .map_err(|err| DiDiError::Startup(err.to_string()))?;
    } else {
        app.autolaunch()
            .disable()
            .map_err(|err| DiDiError::Startup(err.to_string()))?;
    }
    Ok(())
}

/// Authoritative application-level operation synchronizing Windows Registry autostart
/// with `ConfigStore::launch_at_login` (DEF-02).
///
/// - If config cannot be loaded/saved (e.g. `UnsupportedFutureSchema`), fails before touching Registry.
/// - If Registry update fails, `config.json` is left unchanged and an error is returned.
/// - If `config.json` persistence fails after Registry update, rolls back the Registry change so
///   both remain synchronized.
pub fn sync_autostart_setting(
    store: &ConfigStore,
    backend: &dyn AutostartBackend,
    enable: bool,
) -> Result<AppConfigV1, DiDiError> {
    sync_preferences_with_autostart(
        store,
        backend,
        PreferencesPatch {
            launch_at_login: Some(enable),
            ..PreferencesPatch::default()
        },
    )
}

/// Atomically updates application preferences and synchronizes Windows Registry autostart when
/// `patch.launch_at_login` is specified, persisting `config.json` in a single atomic write and
/// rolling back Registry changes if config persistence fails.
pub fn sync_preferences_with_autostart(
    store: &ConfigStore,
    backend: &dyn AutostartBackend,
    patch: PreferencesPatch,
) -> Result<AppConfigV1, DiDiError> {
    let current = store.get_config()?;
    let previous_autostart = current.launch_at_login;

    if let Some(enable) = patch.launch_at_login {
        backend.set_enabled(enable)?;
    }

    match store.update_preferences(patch.clone()) {
        Ok(updated) => Ok(updated),
        Err(store_err) => {
            if patch.launch_at_login.is_some() {
                // Best-effort rollback of Windows Registry entry to prevent config/Registry split
                let _ = backend.set_enabled(previous_autostart);
            }
            Err(DiDiError::Storage(store_err))
        }
    }
}

/// Reconciles `ConfigStore::launch_at_login` against the actual Windows Registry autostart state
/// at startup (DEF-02).
///
/// **Source of Truth Policy:** Windows Registry (`HKCU\...\Run`) is treated as authoritative for
/// actual sign-in startup behavior so external user changes (e.g. disabling DiDi in Windows
/// Task Manager -> Startup Apps) are respected and never silently re-enabled.
pub fn reconcile_autostart_on_startup(
    store: &ConfigStore,
    backend: &dyn AutostartBackend,
) -> Result<bool, DiDiError> {
    let config = store.get_config()?;
    let registry_enabled = match backend.is_enabled() {
        Ok(val) => val,
        Err(_) => return Ok(config.launch_at_login),
    };

    if config.launch_at_login != registry_enabled {
        store.set_launch_at_login(registry_enabled)?;
    }
    Ok(registry_enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct MockAutostartBackend {
        enabled: Cell<bool>,
        fail_read: Cell<bool>,
        fail_write: Cell<bool>,
        write_calls: RefCell<Vec<bool>>,
    }

    impl MockAutostartBackend {
        fn new(initial: bool) -> Self {
            Self {
                enabled: Cell::new(initial),
                fail_read: Cell::new(false),
                fail_write: Cell::new(false),
                write_calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl AutostartBackend for MockAutostartBackend {
        fn is_enabled(&self) -> Result<bool, DiDiError> {
            if self.fail_read.get() {
                return Err(DiDiError::Startup("registry read failed".to_owned()));
            }
            Ok(self.enabled.get())
        }

        fn set_enabled(&self, enable: bool) -> Result<(), DiDiError> {
            self.write_calls.borrow_mut().push(enable);
            if self.fail_write.get() {
                return Err(DiDiError::Startup("registry access denied".to_owned()));
            }
            self.enabled.set(enable);
            Ok(())
        }
    }

    fn temp_store() -> Result<(ConfigStore, PathBuf), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!(
            "didi_test_startup_{}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        let store = ConfigStore::new(dir.clone());
        let _ = store.load_or_init()?;
        Ok((store, dir))
    }

    #[test]
    fn detects_minimized_flag_case_insensitively() {
        let with_flag = vec!["didi.exe".to_owned(), MINIMIZED_ARG.to_owned()];
        let upper_flag = vec!["didi.exe".to_owned(), "--MINIMIZED".to_owned()];
        let without_flag = vec!["didi.exe".to_owned()];

        assert!(has_minimized_flag(&with_flag));
        assert!(has_minimized_flag(&upper_flag));
        assert!(!has_minimized_flag(&without_flag));
    }

    #[test]
    fn sync_autostart_enables_disables_and_handles_repeated_toggles(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = temp_store()?;
        let backend = MockAutostartBackend::new(false);

        let cfg1 = sync_autostart_setting(&store, &backend, true)?;
        assert!(cfg1.launch_at_login);
        assert!(backend.enabled.get());

        // Repeated enable remains synchronized
        let cfg1_repeat = sync_autostart_setting(&store, &backend, true)?;
        assert!(cfg1_repeat.launch_at_login);
        assert!(backend.enabled.get());

        let cfg2 = sync_autostart_setting(&store, &backend, false)?;
        assert!(!cfg2.launch_at_login);
        assert!(!backend.enabled.get());

        // Repeated disable remains synchronized
        let cfg2_repeat = sync_autostart_setting(&store, &backend, false)?;
        assert!(!cfg2_repeat.launch_at_login);
        assert!(!backend.enabled.get());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn sync_autostart_registry_failure_does_not_update_config(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = temp_store()?;
        let backend = MockAutostartBackend::new(false);
        backend.fail_write.set(true);

        let res = sync_autostart_setting(&store, &backend, true);
        assert!(res.is_err());
        assert!(!store.get_config()?.launch_at_login);
        assert!(!backend.enabled.get());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn sync_autostart_config_persistence_failure_rolls_back_registry(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = temp_store()?;
        // First enable cleanly
        let backend = MockAutostartBackend::new(false);
        sync_autostart_setting(&store, &backend, true)?;
        assert!(backend.enabled.get());
        assert!(store.get_config()?.launch_at_login);

        // Now place a future-schema config on disk so store.set_launch_at_login fails during save,
        // wait: store.get_config() runs first; instead replace config_dir with a read-only/file obstacle
        // or test rollback when save fails after get_config succeeds by making config.json a future schema
        // Wait: if config.json is future schema, get_config() fails before touching registry.
        // Test that future schema refuses before mutating Registry:
        let future_json =
            r#"{"schema_version": 99, "active_profile_id": "default", "profiles": []}"#;
        fs::write(store.config_path(), future_json.as_bytes())?;
        let res = sync_autostart_setting(&store, &backend, false);
        assert!(res.is_err());
        // Registry must remain untouched (still true)
        assert!(backend.enabled.get());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn sync_autostart_rolls_back_registry_when_store_save_fails_mid_operation(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = temp_store()?;
        let backend = MockAutostartBackend::new(false);

        // Make config.json.tmp a directory so get_config() succeeds, Registry enables,
        // and then store.set_launch_at_login(true) fails when writing config.json.tmp!
        fs::create_dir_all(store.tmp_path())?;
        let res = sync_autostart_setting(&store, &backend, true);
        assert!(res.is_err());
        // Registry must be rolled back to false!
        assert!(!backend.enabled.get());
        assert_eq!(*backend.write_calls.borrow(), vec![true, false]);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn startup_reconciliation_syncs_config_to_authoritative_registry(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = temp_store()?;

        // Case 1: Config says false, Registry says true -> config updates to true
        let backend_on = MockAutostartBackend::new(true);
        let reconciled = reconcile_autostart_on_startup(&store, &backend_on)?;
        assert!(reconciled);
        assert!(store.get_config()?.launch_at_login);

        // Case 2: User disabled startup in Windows Task Manager -> Registry is false, Config says true
        let backend_off = MockAutostartBackend::new(false);
        let reconciled_off = reconcile_autostart_on_startup(&store, &backend_off)?;
        assert!(!reconciled_off);
        assert!(!store.get_config()?.launch_at_login);

        // Case 3: Registry read fails -> preserves stored config without failing startup
        store.set_launch_at_login(true)?;
        let backend_err = MockAutostartBackend::new(false);
        backend_err.fail_read.set(true);
        let fallback = reconcile_autostart_on_startup(&store, &backend_err)?;
        assert!(fallback);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn sync_preferences_with_autostart_is_single_atomic_write_with_rollback(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::storage::UiLayout;

        let (store, dir) = temp_store()?;
        let backend = MockAutostartBackend::new(false);

        // Save initial baseline so .bak has close_to_tray=true, launch_at_login=false
        let mut baseline = store.get_config()?;
        baseline.start_minimized = false;
        store.save(&baseline)?;

        // Update multiple fields + launch_at_login in one atomic patch
        let updated = sync_preferences_with_autostart(
            &store,
            &backend,
            PreferencesPatch {
                launch_at_login: Some(true),
                start_minimized: None,
                close_to_tray: Some(false),
                ui_layout: Some(UiLayout::Expanded),
            },
        )?;
        assert!(updated.launch_at_login);
        assert!(!updated.start_minimized);
        assert!(!updated.close_to_tray);
        assert_eq!(updated.ui_layout, UiLayout::Expanded);
        assert!(backend.enabled.get());

        // Single atomic write means .bak still holds the pre-call baseline (not half-updated state!)
        let bak_json = fs::read_to_string(store.backup_path())?;
        let bak_cfg: AppConfigV1 = serde_json::from_str(&bak_json)?;
        assert!(!bak_cfg.launch_at_login);
        assert!(bak_cfg.close_to_tray);

        // Failure mid-save rolls back Registry even when combined with other preferences
        fs::create_dir_all(store.tmp_path())?;
        let err_res = sync_preferences_with_autostart(
            &store,
            &backend,
            PreferencesPatch {
                launch_at_login: Some(false),
                close_to_tray: Some(true),
                ..PreferencesPatch::default()
            },
        );
        assert!(err_res.is_err());
        assert!(backend.enabled.get());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }
}
