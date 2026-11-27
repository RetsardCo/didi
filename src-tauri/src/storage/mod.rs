pub mod errors;
pub mod migration;
pub mod schema;
pub mod store;

pub use errors::StorageError;
pub use migration::{inspect_schema_version, parse_and_migrate_config};
pub use schema::{
    AppConfigV1, ExportBundleV1, ExportProfileV1, ImportConflictStrategy, ImportPreview,
    ProfileConflict, ProfileSummary, ProfileV1, TimestampMode, UiLayout, CURRENT_BUNDLE_VERSION,
    CURRENT_SCHEMA_VERSION, MAX_IMPORT_BYTES,
};
pub use store::{
    ConfigStore, PreferencesPatch, StoreLoadStatus, BACKUP_FILE_NAME, BACKUP_TMP_FILE_NAME,
    CONFIG_FILE_NAME, MAX_QUARANTINE_FILES, TMP_FILE_NAME,
};
