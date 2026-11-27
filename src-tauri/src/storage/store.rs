use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::presence::model::ApplicationId;
use crate::presence::validation::{ValidationError, MAX_PROFILES_COUNT};
use crate::storage::errors::StorageError;
use crate::storage::migration::parse_and_migrate_config;
use crate::storage::schema::{
    AppConfigV1, ExportBundleV1, ImportConflictStrategy, ImportPreview, ProfileConflict,
    ProfileSummary, ProfileV1, UiLayout, CURRENT_BUNDLE_VERSION, MAX_IMPORT_BYTES,
};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const BACKUP_FILE_NAME: &str = "config.json.bak";
pub const BACKUP_TMP_FILE_NAME: &str = "config.json.bak.tmp";
pub const TMP_FILE_NAME: &str = "config.json.tmp";
pub const MAX_QUARANTINE_FILES: usize = 3;
const RENAME_MAX_ATTEMPTS: usize = 5;
const RENAME_RETRY_DELAY: Duration = Duration::from_millis(15);

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferencesPatch {
    pub launch_at_login: Option<bool>,
    pub start_minimized: Option<bool>,
    pub close_to_tray: Option<bool>,
    pub ui_layout: Option<UiLayout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLoadStatus {
    Normal,
    CreatedDefault,
    RecoveredFromBackup { quarantined_file: String },
    ResetToDefaultAfterCorruption { quarantined_file: String },
}

pub struct ConfigStore {
    config_dir: PathBuf,
    write_lock: Mutex<()>,
}

impl ConfigStore {
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dir,
            write_lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    #[must_use]
    pub fn backup_path(&self) -> PathBuf {
        self.config_dir.join(BACKUP_FILE_NAME)
    }

    #[must_use]
    pub fn backup_tmp_path(&self) -> PathBuf {
        self.config_dir.join(BACKUP_TMP_FILE_NAME)
    }

    #[must_use]
    pub fn tmp_path(&self) -> PathBuf {
        self.config_dir.join(TMP_FILE_NAME)
    }

    /// Loads active configuration with full corruption recovery and future-schema guards (ADR D-020).
    pub fn load_or_init(&self) -> Result<(AppConfigV1, StoreLoadStatus), StorageError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| StorageError::Io(std::io::Error::other("lock poisoned")))?;

        let config_path = self.config_path();

        if !config_path.exists() {
            let default_config = AppConfigV1::default_config();
            self.save_internal(&default_config)?;
            return Ok((default_config, StoreLoadStatus::CreatedDefault));
        }

        let raw_bytes = fs::read(&config_path)?;

        match parse_and_migrate_config(&raw_bytes) {
            Ok(config) => Ok((config, StoreLoadStatus::Normal)),
            Err(StorageError::UnsupportedFutureSchema {
                found,
                supported_max,
            }) => {
                // Must NOT quarantine or overwrite future schema versions (ADR D-020, Section 4.4)
                Err(StorageError::UnsupportedFutureSchema {
                    found,
                    supported_max,
                })
            }
            Err(_corrupt_err) => {
                // Main config is corrupt -> quarantine it
                let quarantined_path = self.quarantine_file(&config_path)?;
                let quarantined_str = quarantined_path.to_string_lossy().into_owned();

                // Attempt recovery from .bak
                let backup_path = self.backup_path();
                if backup_path.exists() {
                    if let Ok(bak_bytes) = fs::read(&backup_path) {
                        if let Ok(recovered_config) = parse_and_migrate_config(&bak_bytes) {
                            // Restore valid backup to main config.json
                            self.save_internal(&recovered_config)?;
                            return Ok((
                                recovered_config,
                                StoreLoadStatus::RecoveredFromBackup {
                                    quarantined_file: quarantined_str,
                                },
                            ));
                        }
                        // Backup is also corrupt -> quarantine backup too
                        let _ = self.quarantine_file(&backup_path);
                    }
                }

                // Both main and backup failed -> initialize safe defaults
                let default_config = AppConfigV1::default_config();
                self.save_internal(&default_config)?;
                Ok((
                    default_config,
                    StoreLoadStatus::ResetToDefaultAfterCorruption {
                        quarantined_file: quarantined_str,
                    },
                ))
            }
        }
    }

    /// Atomically persists `AppConfigV1` following ADR D-020 crash-safe protocol.
    pub fn save(&self, config: &AppConfigV1) -> Result<(), StorageError> {
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| StorageError::Io(std::io::Error::other("lock poisoned")))?;
        self.save_internal(config)
    }

    fn save_internal(&self, config: &AppConfigV1) -> Result<(), StorageError> {
        // 1. Validate in-memory
        config.validate().map_err(StorageError::Validation)?;

        // 2. Serialize pretty UTF-8 JSON
        let serialized = serde_json::to_vec_pretty(config)?;

        // 3. Ensure parent directory exists
        fs::create_dir_all(&self.config_dir)?;

        let tmp_path = self.tmp_path();
        let config_path = self.config_path();
        let backup_path = self.backup_path();
        let backup_tmp_path = self.backup_tmp_path();

        // 4. Write to .tmp in same directory, flush and sync_all
        write_atomic_temp_file(&tmp_path, &serialized)?;

        // 5. If existing config.json exists:
        //    a) Refuse to overwrite if it contains an unsupported future or malformed schema version (ADR D-020).
        //    b) Only after verifying existing config.json is valid, write config.json.bak atomically
        //       via config.json.bak.tmp + flush() + sync_all() + rename so corrupt data never overwrites a good backup.
        if config_path.exists() {
            let existing_bytes = match fs::read(&config_path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(StorageError::Io(err));
                }
            };

            match parse_and_migrate_config(&existing_bytes) {
                Ok(_) => {
                    if write_atomic_temp_file(&backup_tmp_path, &existing_bytes).is_ok() {
                        if rename_with_retry(&backup_tmp_path, &backup_path).is_err() {
                            let _ = fs::remove_file(&backup_tmp_path);
                        }
                    } else {
                        let _ = fs::remove_file(&backup_tmp_path);
                    }
                }
                Err(StorageError::UnsupportedFutureSchema {
                    found,
                    supported_max,
                }) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(StorageError::UnsupportedFutureSchema {
                        found,
                        supported_max,
                    });
                }
                Err(StorageError::InvalidSchemaVersion) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(StorageError::InvalidSchemaVersion);
                }
                Err(_) => {}
            }
        }

        // 6. Replace config.json with config.json.tmp using bounded sharing-violation retries
        if let Err(err) = rename_with_retry(&tmp_path, &config_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(StorageError::Io(err));
        }

        Ok(())
    }

    /// Quarantines a corrupt file to `config.json.corrupt.<timestamp>`, bounding quarantine count to 3
    /// and avoiding sub-millisecond filename collisions.
    fn quarantine_file(&self, source_path: &Path) -> Result<PathBuf, StorageError> {
        self.cleanup_old_quarantine_files()?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let filename = source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.json");

        let mut quarantine_path = self
            .config_dir
            .join(format!("{filename}.corrupt.{timestamp}"));
        let mut suffix: usize = 1;
        while quarantine_path.exists() {
            quarantine_path = self
                .config_dir
                .join(format!("{filename}.corrupt.{timestamp}-{suffix}"));
            suffix = suffix.saturating_add(1);
        }

        rename_with_retry(source_path, &quarantine_path)?;
        Ok(quarantine_path)
    }

    fn cleanup_old_quarantine_files(&self) -> Result<(), StorageError> {
        if !self.config_dir.exists() {
            return Ok(());
        }

        let mut corrupt_entries = Vec::new();
        for entry in fs::read_dir(&self.config_dir)? {
            let entry = entry?;
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                if file_name.contains(".corrupt.") {
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    corrupt_entries.push((path, mtime));
                }
            }
        }

        // Keep at most (MAX_QUARANTINE_FILES - 1) existing so adding new one stays <= MAX_QUARANTINE_FILES
        if corrupt_entries.len() >= MAX_QUARANTINE_FILES {
            corrupt_entries.sort_by(|(p1, m1), (p2, m2)| m1.cmp(m2).then_with(|| p1.cmp(p2)));
            let excess = corrupt_entries.len() - (MAX_QUARANTINE_FILES - 1);
            for (path, _) in corrupt_entries.iter().take(excess) {
                let _ = fs::remove_file(path);
            }
        }

        Ok(())
    }

    /// Retrieves current configuration snapshot.
    pub fn get_config(&self) -> Result<AppConfigV1, StorageError> {
        let (config, _) = self.load_or_init()?;
        Ok(config)
    }

    /// Saves or updates a profile in configuration after normalizing and validating inputs.
    pub fn save_profile(&self, mut profile: ProfileV1) -> Result<AppConfigV1, StorageError> {
        profile
            .normalize_and_validate()
            .map_err(StorageError::Validation)?;

        let mut config = self.get_config()?;
        if let Some(existing) = config.find_profile_mut(&profile.id) {
            *existing = profile;
        } else {
            if config.profiles.len() >= MAX_PROFILES_COUNT {
                return Err(StorageError::Validation(ValidationError::TooManyProfiles {
                    max: MAX_PROFILES_COUNT,
                    actual: config.profiles.len() + 1,
                }));
            }
            config.profiles.push(profile);
        }

        self.save(&config)?;
        Ok(config)
    }

    /// Deletes a non-last profile.
    pub fn delete_profile(&self, profile_id: &str) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        if config.profiles.len() <= 1 {
            return Err(StorageError::CannotDeleteLastProfile);
        }

        let index = config
            .profiles
            .iter()
            .position(|p| p.id == profile_id)
            .ok_or_else(|| StorageError::ProfileNotFound(profile_id.to_owned()))?;

        config.profiles.remove(index);

        // If active profile was deleted, switch active profile to first available
        if config.active_profile_id == profile_id {
            if let Some(first) = config.profiles.first() {
                config.active_profile_id = first.id.clone();
            }
        }

        self.save(&config)?;
        Ok(config)
    }

    /// Switches the active profile ID.
    pub fn set_active_profile(&self, profile_id: &str) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        if !config.profiles.iter().any(|p| p.id == profile_id) {
            return Err(StorageError::ProfileNotFound(profile_id.to_owned()));
        }

        config.active_profile_id = profile_id.to_owned();
        self.save(&config)?;
        Ok(config)
    }

    /// Updates the default Application ID, normalizing surrounding whitespace before storage.
    pub fn set_default_application_id(
        &self,
        app_id: Option<String>,
    ) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        let normalized_id = match app_id {
            Some(raw) => {
                let parsed = ApplicationId::parse(&raw)
                    .map_err(|err| StorageError::Validation(ValidationError::ApplicationId(err)))?;
                Some(parsed.as_str().to_owned())
            }
            None => None,
        };

        config.default_application_id = normalized_id;
        self.save(&config)?;
        Ok(config)
    }

    /// Toggles presence enabled preference.
    pub fn set_presence_enabled(&self, enabled: bool) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        config.presence_enabled = enabled;
        self.save(&config)?;
        Ok(config)
    }

    /// Updates `launch_at_login` preference atomically.
    pub fn set_launch_at_login(&self, enabled: bool) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        config.launch_at_login = enabled;
        self.save(&config)?;
        Ok(config)
    }

    /// Updates one or more application preferences while preserving unspecified fields.
    pub fn update_preferences(&self, patch: PreferencesPatch) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        if let Some(v) = patch.launch_at_login {
            config.launch_at_login = v;
        }
        if let Some(v) = patch.start_minimized {
            config.start_minimized = v;
        }
        if let Some(v) = patch.close_to_tray {
            config.close_to_tray = v;
        }
        if let Some(v) = patch.ui_layout {
            config.ui_layout = v;
        }
        self.save(&config)?;
        Ok(config)
    }

    /// Generates a credential-free export bundle.
    pub fn export_bundle(
        &self,
        profile_ids: Option<Vec<String>>,
    ) -> Result<ExportBundleV1, StorageError> {
        let config = self.get_config()?;
        let profiles_to_export: Vec<ProfileV1> = match profile_ids {
            Some(ids) => {
                for id in &ids {
                    if !config.profiles.iter().any(|p| &p.id == id) {
                        return Err(StorageError::ProfileNotFound(id.clone()));
                    }
                }
                config
                    .profiles
                    .into_iter()
                    .filter(|p| ids.contains(&p.id))
                    .collect()
            }
            None => config.profiles,
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(ExportBundleV1::from_profiles(&profiles_to_export, now))
    }

    /// Parses and validates an import bundle, returning both the summary preview and parsed profiles
    /// without modifying configuration.
    pub fn preview_and_parse_import(
        &self,
        json_bytes: &[u8],
    ) -> Result<(ImportPreview, Vec<ProfileV1>), StorageError> {
        let (bundle_version, validated_profiles) = parse_and_validate_import_bundle(json_bytes)?;

        let current_config = self.get_config()?;
        let mut new_profiles = Vec::new();
        let mut conflicting_profiles = Vec::new();

        for profile in &validated_profiles {
            if let Some(existing) = current_config.find_profile(&profile.id) {
                conflicting_profiles.push(ProfileConflict {
                    existing_id: existing.id.clone(),
                    existing_name: existing.name.clone(),
                    imported_id: profile.id.clone(),
                    imported_name: profile.name.clone(),
                });
            } else {
                new_profiles.push(ProfileSummary::from(profile));
            }
        }

        let existing_count = current_config.profiles.len();
        let would_exceed_max_profiles =
            existing_count.saturating_add(new_profiles.len()) > MAX_PROFILES_COUNT;
        let would_exceed_max_profiles_keep_both =
            existing_count.saturating_add(validated_profiles.len()) > MAX_PROFILES_COUNT;

        let preview = ImportPreview {
            bundle_version,
            total_profiles: validated_profiles.len(),
            new_profiles,
            conflicting_profiles,
            would_exceed_max_profiles,
            would_exceed_max_profiles_keep_both,
        };

        Ok((preview, validated_profiles))
    }

    /// Parses and validates an import bundle, generating a preview without modifying configuration.
    pub fn preview_import(&self, json_bytes: &[u8]) -> Result<ImportPreview, StorageError> {
        self.preview_and_parse_import(json_bytes)
            .map(|(preview, _)| preview)
    }

    /// Applies an import bundle using the specified conflict resolution strategy.
    pub fn apply_import(
        &self,
        json_bytes: &[u8],
        strategy: ImportConflictStrategy,
    ) -> Result<AppConfigV1, StorageError> {
        let (_bundle_version, validated_profiles) = parse_and_validate_import_bundle(json_bytes)?;
        self.apply_validated_profiles(validated_profiles, strategy)
    }

    /// Applies previously validated import profiles with deterministic, ordering-independent
    /// `KeepBoth` ID disambiguation.
    pub fn apply_validated_profiles(
        &self,
        validated_profiles: Vec<ProfileV1>,
        strategy: ImportConflictStrategy,
    ) -> Result<AppConfigV1, StorageError> {
        let mut config = self.get_config()?;
        let initial_existing_len = config.profiles.len();

        // Reserve all existing IDs AND all bundle IDs up front so KeepBoth renaming
        // is completely independent of bundle profile ordering.
        let mut reserved_ids: HashSet<String> = config
            .profiles
            .iter()
            .map(|p| p.id.clone())
            .chain(validated_profiles.iter().map(|p| p.id.clone()))
            .collect();

        for mut profile in validated_profiles {
            let conflict_idx = config.profiles[..initial_existing_len]
                .iter()
                .position(|p| p.id == profile.id);

            match (conflict_idx, strategy) {
                (Some(idx), ImportConflictStrategy::Overwrite) => {
                    config.profiles[idx] = profile;
                }
                (Some(_), ImportConflictStrategy::Skip) => {
                    // Skip conflicting profile
                }
                (Some(_), ImportConflictStrategy::KeepBoth) => {
                    let new_id = generate_unique_profile_id(&profile.id, &reserved_ids);
                    reserved_ids.insert(new_id.clone());
                    profile.id = new_id;

                    // Safely disambiguate name while strictly respecting MAX_PROFILE_NAME_LEN (64)
                    let name_suffix = " (Imported)";
                    let max_name_base_len = crate::presence::validation::MAX_PROFILE_NAME_LEN
                        .saturating_sub(name_suffix.len());
                    let base_name: String = profile.name.chars().take(max_name_base_len).collect();
                    profile.name = format!("{base_name}{name_suffix}");

                    config.profiles.push(profile);
                }
                (None, _) => {
                    config.profiles.push(profile);
                }
            }
        }

        if config.profiles.len() > MAX_PROFILES_COUNT {
            return Err(StorageError::Validation(ValidationError::TooManyProfiles {
                max: MAX_PROFILES_COUNT,
                actual: config.profiles.len(),
            }));
        }

        self.save(&config)?;
        Ok(config)
    }
}

fn parse_and_validate_import_bundle(
    json_bytes: &[u8],
) -> Result<(u32, Vec<ProfileV1>), StorageError> {
    if json_bytes.len() > MAX_IMPORT_BYTES {
        return Err(StorageError::ImportTooLarge {
            size_bytes: json_bytes.len(),
            max_bytes: MAX_IMPORT_BYTES,
        });
    }

    let bundle: ExportBundleV1 = serde_json::from_slice(json_bytes)
        .map_err(|e| StorageError::ImportSchemaInvalid(e.to_string()))?;

    if bundle.bundle_version != CURRENT_BUNDLE_VERSION {
        return Err(StorageError::UnsupportedBundleVersion {
            found: bundle.bundle_version,
            supported: CURRENT_BUNDLE_VERSION,
        });
    }

    let mut validated_profiles = Vec::with_capacity(bundle.profiles.len());
    let mut seen_bundle_ids = HashSet::new();
    for export_p in bundle.profiles {
        let mut profile: ProfileV1 = export_p.into();
        profile
            .normalize_and_validate()
            .map_err(StorageError::Validation)?;
        if !seen_bundle_ids.insert(profile.id.clone()) {
            return Err(StorageError::Validation(
                ValidationError::DuplicateProfileId(profile.id),
            ));
        }
        validated_profiles.push(profile);
    }

    Ok((bundle.bundle_version, validated_profiles))
}

fn generate_unique_profile_id(original_id: &str, reserved_ids: &HashSet<String>) -> String {
    let mut counter: usize = 2;
    loop {
        let suffix = format!("-{counter}");
        let max_id_base_len =
            crate::presence::validation::MAX_PROFILE_ID_LEN.saturating_sub(suffix.len());
        let truncated: String = original_id.chars().take(max_id_base_len).collect();
        let trimmed_base = truncated.trim_end_matches('-');
        let base = if trimmed_base.is_empty() {
            "p"
        } else {
            trimmed_base
        };
        let candidate = format!("{base}{suffix}");
        if !reserved_ids.contains(&candidate) {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

fn write_atomic_temp_file(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn is_transient_windows_rename_error(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(32 | 5))
}

fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err)
                if is_transient_windows_rename_error(&err) && attempt + 1 < RENAME_MAX_ATTEMPTS =>
            {
                attempt += 1;
                std::thread::sleep(RENAME_RETRY_DELAY);
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn create_test_store() -> Result<(ConfigStore, PathBuf), Box<dyn std::error::Error>> {
        let unique_dir = std::env::temp_dir().join(format!(
            "didi_test_store_{}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            std::process::id()
        ));
        fs::create_dir_all(&unique_dir)?;
        let store = ConfigStore::new(unique_dir.clone());
        Ok((store, unique_dir))
    }

    #[test]
    fn store_initializes_default_config_when_missing() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (config, status) = store.load_or_init()?;
        assert_eq!(status, StoreLoadStatus::CreatedDefault);
        assert_eq!(config.schema_version, 1);
        assert!(store.config_path().exists());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_atomic_write_creates_backup_and_persists_changes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (mut config, _) = store.load_or_init()?;

        config.ui_layout = crate::storage::schema::UiLayout::Expanded;
        store.save(&config)?;

        assert!(store.backup_path().exists());
        assert!(!store.tmp_path().exists());

        let (reloaded, status) = store.load_or_init()?;
        assert_eq!(status, StoreLoadStatus::Normal);
        assert_eq!(
            reloaded.ui_layout,
            crate::storage::schema::UiLayout::Expanded
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_quarantines_corrupt_config_and_recovers_from_backup(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (mut config, _) = store.load_or_init()?;

        config.ui_layout = crate::storage::schema::UiLayout::Expanded;
        store.save(&config)?;
        assert!(store.backup_path().exists());

        // Save again so that `ui_layout: Expanded` becomes the backup in config.json.bak
        config.start_minimized = false;
        store.save(&config)?;

        // Corrupt main config.json with truncated garbage
        fs::write(
            store.config_path(),
            b"{\"schema_version\": 1, \"corrupt\": ",
        )?;

        let (recovered, status) = store.load_or_init()?;
        match status {
            StoreLoadStatus::RecoveredFromBackup { quarantined_file } => {
                assert!(quarantined_file.contains("corrupt"));
            }
            StoreLoadStatus::Normal
            | StoreLoadStatus::CreatedDefault
            | StoreLoadStatus::ResetToDefaultAfterCorruption { .. } => {
                return Err("expected RecoveredFromBackup".into());
            }
        }
        assert_eq!(
            recovered.ui_layout,
            crate::storage::schema::UiLayout::Expanded
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_quarantines_both_corrupt_and_resets_to_defaults(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        fs::write(store.config_path(), b"garbage-main")?;
        fs::write(store.backup_path(), b"garbage-bak")?;

        let (reset_config, status) = store.load_or_init()?;
        match status {
            StoreLoadStatus::ResetToDefaultAfterCorruption { quarantined_file } => {
                assert!(quarantined_file.contains("corrupt"));
            }
            StoreLoadStatus::Normal
            | StoreLoadStatus::CreatedDefault
            | StoreLoadStatus::RecoveredFromBackup { .. } => {
                return Err("expected ResetToDefaultAfterCorruption".into());
            }
        }
        assert_eq!(reset_config.schema_version, 1);
        assert_eq!(reset_config.active_profile_id, "default");

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_bounds_quarantine_files_to_three() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;

        // Create 5 fake corrupt files
        for i in 1..=5 {
            let p = dir.join(format!("config.json.corrupt.{i}"));
            fs::write(&p, b"bad")?;
            std::thread::sleep(Duration::from_millis(15));
        }

        let dummy_corrupt = dir.join("config.json");
        fs::write(&dummy_corrupt, b"bad-main")?;
        let _ = store.quarantine_file(&dummy_corrupt);

        let mut count = 0;
        for entry in fs::read_dir(&dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.contains(".corrupt.") {
                count += 1;
            }
        }
        assert!(count <= MAX_QUARANTINE_FILES);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_rejects_future_schema_without_overwriting() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        let future_json = r#"{
            "schema_version": 99,
            "default_application_id": null,
            "active_profile_id": "default",
            "presence_enabled": true,
            "launch_at_login": false,
            "start_minimized": true,
            "close_to_tray": true,
            "ui_layout": "compact",
            "profiles": []
        }"#;

        fs::write(store.config_path(), future_json.as_bytes())?;

        let load_res = store.load_or_init();
        match load_res {
            Err(StorageError::UnsupportedFutureSchema {
                found,
                supported_max,
            }) => {
                assert_eq!(found, 99);
                assert_eq!(supported_max, 1);
            }
            Ok(_) | Err(_) => {
                return Err("expected UnsupportedFutureSchema error".into());
            }
        }

        // Verify config.json was NOT overwritten or quarantined
        let content = fs::read_to_string(store.config_path())?;
        assert!(content.contains("\"schema_version\": 99"));

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn profile_crud_operations() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        // Create new profile
        let mut new_p = ProfileV1::default_profile();
        new_p.id = "coding".to_owned();
        new_p.name = "Coding Profile".to_owned();
        new_p.details = Some("Writing Rust".to_owned());

        let updated = store.save_profile(new_p)?;
        assert_eq!(updated.profiles.len(), 2);
        assert!(updated.find_profile("coding").is_some());

        // Set active profile
        let switched = store.set_active_profile("coding")?;
        assert_eq!(switched.active_profile_id, "coding");

        // Delete default profile
        let after_delete = store.delete_profile("default")?;
        assert_eq!(after_delete.profiles.len(), 1);
        assert_eq!(after_delete.active_profile_id, "coding");

        // Attempt to delete last remaining profile -> must fail
        match store.delete_profile("coding") {
            Err(StorageError::CannotDeleteLastProfile) => {}
            Ok(_) | Err(_) => {
                return Err("expected CannotDeleteLastProfile error".into());
            }
        }

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn import_export_and_conflict_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        // Export bundle
        let bundle = store.export_bundle(None)?;
        let bundle_bytes = serde_json::to_vec_pretty(&bundle)?;

        // Preview import
        let preview = store.preview_import(&bundle_bytes)?;
        assert_eq!(preview.conflicting_profiles.len(), 1);
        assert_eq!(preview.new_profiles.len(), 0);

        // Apply with KeepBoth
        let merged = store.apply_import(&bundle_bytes, ImportConflictStrategy::KeepBoth)?;
        assert_eq!(merged.profiles.len(), 2);
        assert!(merged.find_profile("default-2").is_some());

        // Verify import cap <= 256 KiB
        let oversized = vec![b' '; MAX_IMPORT_BYTES + 1];
        assert!(store.preview_import(&oversized).is_err());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_handles_interrupted_write_leftover_tmp_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let (store, dir) = create_test_store()?;
        let (config, _) = store.load_or_init()?;

        // Simulate an interrupted write that left a partial .tmp file behind
        fs::write(store.tmp_path(), b"{\"partial_interrupted_write\": true")?;
        assert!(store.tmp_path().exists());

        // Subsequent valid save must succeed, overwrite .tmp, and preserve config.json integrity
        let mut updated = config;
        updated.ui_layout = crate::storage::schema::UiLayout::Expanded;
        store.save(&updated)?;

        assert!(!store.tmp_path().exists());
        let (reloaded, _) = store.load_or_init()?;
        assert_eq!(
            reloaded.ui_layout,
            crate::storage::schema::UiLayout::Expanded
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_save_refuses_to_overwrite_future_schema() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (config, _) = store.load_or_init()?;

        // External tool or newer DiDi writes schema_version = 5
        let future_json = r#"{
            "schema_version": 5,
            "default_application_id": null,
            "active_profile_id": "default",
            "presence_enabled": true,
            "launch_at_login": false,
            "start_minimized": true,
            "close_to_tray": true,
            "ui_layout": "compact",
            "profiles": []
        }"#;
        fs::write(store.config_path(), future_json.as_bytes())?;

        // Attempting to save config v1 must be rejected and must NOT overwrite the future file
        let save_res = store.save(&config);
        match save_res {
            Err(StorageError::UnsupportedFutureSchema {
                found,
                supported_max,
            }) => {
                assert_eq!(found, 5);
                assert_eq!(supported_max, 1);
            }
            Ok(_) | Err(_) => {
                return Err("expected UnsupportedFutureSchema error on save".into());
            }
        }

        let on_disk = fs::read_to_string(store.config_path())?;
        assert!(on_disk.contains("\"schema_version\": 5"));

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn apply_import_keep_both_handles_max_length_profile_names_and_ids(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        // Create profile with max name (64 chars) and max id (64 chars)
        let max_name = "N".repeat(crate::presence::validation::MAX_PROFILE_NAME_LEN);
        let max_id = "p".repeat(crate::presence::validation::MAX_PROFILE_ID_LEN);

        let mut p = ProfileV1::default_profile();
        p.id = max_id.clone();
        p.name = max_name.clone();
        store.save_profile(p.clone())?;

        // Create an export bundle containing this exact profile
        let bundle = ExportBundleV1::from_profiles(&[p], 1700000000);
        let bundle_bytes = serde_json::to_vec_pretty(&bundle)?;

        // Apply with KeepBoth: must NOT overflow 64 char bounds or fail validation
        let updated = store.apply_import(&bundle_bytes, ImportConflictStrategy::KeepBoth)?;
        assert_eq!(updated.profiles.len(), 3); // default + original + imported

        let imported = updated
            .profiles
            .iter()
            .find(|prof| prof.id != max_id && prof.id.starts_with(&max_id[..50]))
            .ok_or("imported profile not found")?;

        assert!(imported.name.contains("(Imported)"));
        assert!(imported.name.chars().count() <= crate::presence::validation::MAX_PROFILE_NAME_LEN);
        assert!(imported.id.len() <= crate::presence::validation::MAX_PROFILE_ID_LEN);

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn apply_import_rejects_duplicate_ids_within_bundle() -> Result<(), Box<dyn std::error::Error>>
    {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        let mut p1 = ProfileV1::default_profile();
        p1.id = "dupe".to_owned();
        let mut p2 = ProfileV1::default_profile();
        p2.id = "dupe".to_owned();

        let bundle = ExportBundleV1 {
            bundle_version: 1,
            exported_at_unix: 1700000000,
            profiles: vec![
                crate::storage::schema::ExportProfileV1::from(&p1),
                crate::storage::schema::ExportProfileV1::from(&p2),
            ],
        };
        let bundle_bytes = serde_json::to_vec_pretty(&bundle)?;

        assert!(store.preview_import(&bundle_bytes).is_err());
        assert!(store
            .apply_import(&bundle_bytes, ImportConflictStrategy::Overwrite)
            .is_err());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn export_bundle_rejects_nonexistent_requested_profile_ids(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        let res = store.export_bundle(Some(vec!["does_not_exist".to_owned()]));
        match res {
            Err(StorageError::ProfileNotFound(id)) => {
                assert_eq!(id, "does_not_exist");
            }
            Ok(_) | Err(_) => {
                return Err("expected ProfileNotFound error".into());
            }
        }

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_does_not_overwrite_valid_backup_when_disk_config_is_corrupt(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (mut config, _) = store.load_or_init()?;
        config.ui_layout = UiLayout::Expanded;
        store.save(&config)?;

        // Save again so config.json.bak holds valid Expanded config
        config.start_minimized = false;
        store.save(&config)?;
        let original_bak = fs::read_to_string(store.backup_path())?;
        assert!(original_bak.contains("\"expanded\""));

        // Corrupt main config.json on disk, then call store.save(&config)
        fs::write(store.config_path(), b"{corrupt-json-on-disk")?;
        config.close_to_tray = false;
        store.save(&config)?;

        // Backup must STILL contain the last valid config, NOT "{corrupt-json-on-disk"!
        let bak_after = fs::read_to_string(store.backup_path())?;
        assert_eq!(bak_after, original_bak);
        assert!(!store.backup_tmp_path().exists());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_handles_leftover_backup_tmp_file() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (mut config, _) = store.load_or_init()?;

        fs::write(store.backup_tmp_path(), b"{\"partial_bak\": ")?;
        config.ui_layout = UiLayout::Expanded;
        store.save(&config)?;

        assert!(!store.backup_tmp_path().exists());
        assert!(store.backup_path().exists());
        let bak_bytes = fs::read(store.backup_path())?;
        assert!(parse_and_migrate_config(&bak_bytes).is_ok());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_rejects_float_and_string_schema_versions_on_load_and_save(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let (config, _) = store.load_or_init()?;

        for bad_schema in ["2.0", "\"2\""] {
            let content = format!(
                r#"{{"schema_version": {bad_schema}, "active_profile_id": "default", "profiles": []}}"#
            );
            fs::write(store.config_path(), content.as_bytes())?;

            // save() must refuse to overwrite unrecognized/malformed schema version
            let save_res = store.save(&config);
            assert!(matches!(save_res, Err(StorageError::InvalidSchemaVersion)));
            let disk = fs::read_to_string(store.config_path())?;
            assert!(disk.contains(bad_schema));

            // load_or_init() must quarantine (not coerce to v0!) and recover safely
            let (_loaded, status) = store.load_or_init()?;
            assert!(matches!(
                status,
                StoreLoadStatus::RecoveredFromBackup { .. }
                    | StoreLoadStatus::ResetToDefaultAfterCorruption { .. }
            ));
        }

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn quarantine_submillisecond_collisions_preserve_previous_files(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        fs::create_dir_all(&dir)?;

        let p1 = dir.join("config.json");
        fs::write(&p1, b"corrupt-one")?;
        let q1 = store.quarantine_file(&p1)?;

        let p2 = dir.join("config.json");
        fs::write(&p2, b"corrupt-two")?;
        let q2 = store.quarantine_file(&p2)?;

        assert_ne!(q1, q2);
        assert_eq!(fs::read(&q1)?, b"corrupt-one");
        assert_eq!(fs::read(&q2)?, b"corrupt-two");

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_normalizes_app_id_and_profile_fields_on_save() -> Result<(), Box<dyn std::error::Error>>
    {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        let updated =
            store.set_default_application_id(Some("  123456789012345678  ".to_owned()))?;
        assert_eq!(
            updated.default_application_id.as_deref(),
            Some("123456789012345678")
        );

        let mut profile = ProfileV1::default_profile();
        profile.id = "  normalized_id  ".to_owned();
        profile.name = "  Normalized Name  ".to_owned();
        profile.details = Some("  Trimmed Details  ".to_owned());
        profile.state = Some("   ".to_owned());
        profile.large_asset = crate::presence::model::ArtworkRef::PortalKey {
            key: "  My_Upper_Key  ".to_owned(),
        };

        let after_save = store.save_profile(profile)?;
        let saved = after_save
            .find_profile("normalized_id")
            .ok_or("normalized profile missing")?;
        assert_eq!(saved.name, "Normalized Name");
        assert_eq!(saved.details.as_deref(), Some("Trimmed Details"));
        assert_eq!(saved.state, None);
        assert_eq!(
            saved.large_asset,
            crate::presence::model::ArtworkRef::PortalKey {
                key: "my_upper_key".to_owned()
            }
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn import_rejects_unsupported_bundle_versions() -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        for bad_ver in [0u32, 2u32, 99u32] {
            let bundle = ExportBundleV1 {
                bundle_version: bad_ver,
                exported_at_unix: 1700000000,
                profiles: vec![],
            };
            let bytes = serde_json::to_vec(&bundle)?;
            assert!(matches!(
                store.preview_import(&bytes),
                Err(StorageError::UnsupportedBundleVersion {
                    found,
                    supported: 1
                }) if found == bad_ver
            ));
            assert!(matches!(
                store.apply_import(&bytes, ImportConflictStrategy::Skip),
                Err(StorageError::UnsupportedBundleVersion {
                    found,
                    supported: 1
                }) if found == bad_ver
            ));
        }

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn import_keep_both_is_order_independent_and_avoids_generated_collisions(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Test both ["foo", "foo-2"] and ["foo-2", "foo"] when ["foo"] already exists.
        for order in [["foo", "foo-2"], ["foo-2", "foo"]] {
            let (store, dir) = create_test_store()?;
            let _ = store.load_or_init()?;

            let mut existing = ProfileV1::default_profile();
            existing.id = "foo".to_owned();
            existing.name = "Existing Foo".to_owned();
            store.save_profile(existing)?;

            let mut p_a = ProfileV1::default_profile();
            p_a.id = order[0].to_owned();
            p_a.name = format!("Imported {}", order[0]);
            let mut p_b = ProfileV1::default_profile();
            p_b.id = order[1].to_owned();
            p_b.name = format!("Imported {}", order[1]);

            let bundle = ExportBundleV1 {
                bundle_version: 1,
                exported_at_unix: 1700000000,
                profiles: vec![
                    crate::storage::schema::ExportProfileV1::from(&p_a),
                    crate::storage::schema::ExportProfileV1::from(&p_b),
                ],
            };
            let bytes = serde_json::to_vec(&bundle)?;

            let preview = store.preview_import(&bytes)?;
            assert_eq!(preview.conflicting_profiles.len(), 1);
            assert_eq!(preview.conflicting_profiles[0].existing_id, "foo");
            assert_eq!(preview.new_profiles.len(), 1);
            assert_eq!(preview.new_profiles[0].id, "foo-2");

            let merged = store.apply_import(&bytes, ImportConflictStrategy::KeepBoth)?;
            assert_eq!(merged.profiles.len(), 4); // default + existing foo + foo-2 + foo-3
            assert!(merged.find_profile("foo").is_some());
            assert!(merged.find_profile("foo-2").is_some());
            assert!(merged.find_profile("foo-3").is_some());

            let _ = fs::remove_dir_all(&dir);
        }

        // Test generated-ID collision when foo, foo-2, foo-3 already exist
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;
        for id in ["foo", "foo-2", "foo-3"] {
            let mut p = ProfileV1::default_profile();
            p.id = id.to_owned();
            store.save_profile(p)?;
        }
        let mut imp = ProfileV1::default_profile();
        imp.id = "foo".to_owned();
        let bundle = ExportBundleV1::from_profiles(&[imp], 1700000000);
        let bytes = serde_json::to_vec(&bundle)?;
        let merged = store.apply_import(&bytes, ImportConflictStrategy::KeepBoth)?;
        assert!(merged.find_profile("foo-4").is_some());

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn import_preview_and_apply_enforce_50_profile_limit_and_strategies(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        // Fill up to 49 profiles (default + 48)
        for i in 1..=48 {
            let mut p = ProfileV1::default_profile();
            p.id = format!("p-{i}");
            p.name = format!("Profile {i}");
            store.save_profile(p)?;
        }

        let mut p_existing = ProfileV1::default_profile();
        p_existing.id = "default".to_owned();
        p_existing.name = "Overwritten Default".to_owned();

        let mut p_new = ProfileV1::default_profile();
        p_new.id = "p-49".to_owned();
        p_new.name = "Profile 49".to_owned();

        let bundle = ExportBundleV1::from_profiles(&[p_existing, p_new], 1700000000);
        let bytes = serde_json::to_vec(&bundle)?;

        let preview = store.preview_import(&bytes)?;
        assert!(!preview.would_exceed_max_profiles); // 49 + 1 new = 50
        assert!(preview.would_exceed_max_profiles_keep_both); // 49 + 2 = 51 > 50

        // KeepBoth fails due to 50-profile ceiling
        assert!(matches!(
            store.apply_import(&bytes, ImportConflictStrategy::KeepBoth),
            Err(StorageError::Validation(ValidationError::TooManyProfiles {
                max: 50,
                actual: 51
            }))
        ));

        // Skip / Overwrite succeeds at exact 50-profile boundary
        let after_overwrite = store.apply_import(&bytes, ImportConflictStrategy::Overwrite)?;
        assert_eq!(after_overwrite.profiles.len(), 50);
        assert_eq!(
            after_overwrite
                .find_profile("default")
                .map(|p| p.name.as_str()),
            Some("Overwritten Default")
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn store_serialization_roundtrip_preserves_normalized_fields(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (store, dir) = create_test_store()?;
        let _ = store.load_or_init()?;

        let mut profile = ProfileV1::default_profile();
        profile.id = "  round_trip  ".to_owned();
        profile.name = "  Round Trip  ".to_owned();
        profile.details = Some("  Building DiDi  ".to_owned());
        profile.large_asset = crate::presence::model::ArtworkRef::PortalKey {
            key: "  Logo_Main  ".to_owned(),
        };
        store.save_profile(profile)?;

        let (reloaded, status) = store.load_or_init()?;
        assert_eq!(status, StoreLoadStatus::Normal);
        let loaded_p = reloaded
            .find_profile("round_trip")
            .ok_or("round_trip profile missing")?;
        assert_eq!(loaded_p.name, "Round Trip");
        assert_eq!(loaded_p.details.as_deref(), Some("Building DiDi"));
        assert_eq!(
            loaded_p.large_asset,
            crate::presence::model::ArtworkRef::PortalKey {
                key: "logo_main".to_owned()
            }
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn transient_windows_rename_error_detection_matches_sharing_and_access_codes() {
        let sharing = std::io::Error::from_raw_os_error(32);
        let access = std::io::Error::from_raw_os_error(5);
        let not_found = std::io::Error::from_raw_os_error(2);
        assert!(is_transient_windows_rename_error(&sharing));
        assert!(is_transient_windows_rename_error(&access));
        assert!(!is_transient_windows_rename_error(&not_found));
    }
}
