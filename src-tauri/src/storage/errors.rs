use thiserror::Error;

use crate::presence::validation::ValidationError;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization/deserialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("corrupt config file quarantined at '{path}', fallback backup recovered")]
    CorruptConfigRecovered { path: String },

    #[error("corrupt config file quarantined at '{path}', reset to safe defaults")]
    CorruptConfigReset { path: String },

    #[error(
        "unsupported future schema version {found} (maximum supported version is {supported_max})"
    )]
    UnsupportedFutureSchema { found: u32, supported_max: u32 },

    #[error("malformed or invalid schema_version in configuration")]
    InvalidSchemaVersion,

    #[error("unsupported profile bundle version {found} (expected {supported})")]
    UnsupportedBundleVersion { found: u32, supported: u32 },

    #[error("import bundle exceeds maximum size of {max_bytes} bytes (found {size_bytes} bytes)")]
    ImportTooLarge { size_bytes: usize, max_bytes: usize },

    #[error("invalid import schema: {0}")]
    ImportSchemaInvalid(String),

    #[error("profile with ID '{0}' not found")]
    ProfileNotFound(String),

    #[error("duplicate profile ID '{0}'")]
    DuplicateProfileId(String),

    #[error("cannot delete the only remaining profile in configuration")]
    CannotDeleteLastProfile,

    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),
}
