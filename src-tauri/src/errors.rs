use thiserror::Error;

use crate::discord::DiscordError;
use crate::presence::validation::ValidationError;
use crate::storage::StorageError;

#[derive(Debug, Error)]
pub enum DiDiError {
    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("discord adapter error: {0}")]
    Discord(#[from] DiscordError),

    #[error("lifecycle error: {0}")]
    Lifecycle(String),

    #[error("startup error: {0}")]
    Startup(String),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
}

impl DiDiError {
    #[must_use]
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::Validation(_) => "One or more configuration fields are invalid.",
            Self::Discord(err) => err.user_message(),
            Self::Lifecycle(_) => "Unable to complete the requested window or lifecycle action.",
            Self::Startup(_) => {
                "Could not update Windows startup setting. Check permissions and try again."
            }
            Self::Storage(err) => match err {
                StorageError::UnsupportedFutureSchema { .. } => {
                    "Configuration was created by a newer version of DiDi. Please upgrade DiDi."
                }
                StorageError::InvalidSchemaVersion => {
                    "Configuration contains an invalid schema version."
                }
                StorageError::UnsupportedBundleVersion { .. } => {
                    "Imported profile bundle uses an unsupported bundle version."
                }
                StorageError::CorruptConfigRecovered { .. } => {
                    "Configuration was corrupted and safely recovered from backup."
                }
                StorageError::CorruptConfigReset { .. } => {
                    "Configuration was corrupted and reset to defaults. A copy was quarantined."
                }
                StorageError::ImportTooLarge { .. } => {
                    "Imported profile file is too large (maximum 256 KiB)."
                }
                StorageError::ImportSchemaInvalid(_) => {
                    "Imported file is not a valid profile bundle."
                }
                StorageError::CannotDeleteLastProfile => {
                    "Cannot delete the only remaining profile."
                }
                StorageError::ProfileNotFound(_) => "Requested profile was not found.",
                StorageError::Validation(_) => "One or more configuration fields are invalid.",
                StorageError::Io(_)
                | StorageError::Json(_)
                | StorageError::DuplicateProfileId(_) => {
                    "An error occurred while reading or saving configuration."
                }
            },
        }
    }

    /// Returns a user-safe, sanitized error message suitable for frontend display.
    ///
    /// Includes field-level context for `ValidationError` while never exposing raw Windows
    /// filesystem paths, raw Application IDs, serialized `serde_json` internals, or peer RPC strings.
    #[must_use]
    pub fn user_safe_message(&self) -> String {
        match self {
            Self::Validation(err) => format_safe_validation_error(err),
            Self::Storage(StorageError::Validation(err)) => format_safe_validation_error(err),
            _ => self.user_message().to_owned(),
        }
    }
}

fn format_safe_validation_error(err: &ValidationError) -> String {
    match err {
        ValidationError::ApplicationId(app_err) => app_err.to_string(),
        ValidationError::EmptyField(field) => format!("Field '{field}' must not be empty."),
        ValidationError::LengthOutOfRange {
            field,
            min,
            max,
            actual,
        } => format!("Field '{field}' length ({actual}) must be between {min} and {max}."),
        ValidationError::InvalidControlChars(field) => {
            format!("Field '{field}' contains unsupported control characters or line breaks.")
        }
        ValidationError::InvalidUrlScheme(field) => {
            format!("URL in '{field}' must start with https://.")
        }
        ValidationError::InvalidUrlCredentials(field) => {
            format!("URL in '{field}' must not include embedded credentials.")
        }
        ValidationError::DisallowedUrlHost(field) => {
            format!("URL in '{field}' must target a valid public domain or address.")
        }
        ValidationError::InvalidPortalKey(_) => {
            "Portal asset key must be 1..=300 lowercase letters, digits, '-' or '_'.".to_owned()
        }
        ValidationError::TooManyButtons { max, actual } => {
            format!("Too many buttons ({actual}); maximum is {max}.")
        }
        ValidationError::TooManyProfiles { max, actual } => {
            format!("Profile limit exceeded ({actual}/{max}).")
        }
        ValidationError::NoProfiles => "At least one profile is required.".to_owned(),
        ValidationError::DuplicateProfileId(_) => {
            "A profile with that ID already exists.".to_owned()
        }
        ValidationError::ActiveProfileNotFound(_) => {
            "Selected active profile was not found.".to_owned()
        }
        ValidationError::InvalidProfileIdFormat(_) => {
            "Profile ID must be 1..=64 lowercase letters, digits, '-' or '_'.".to_owned()
        }
        ValidationError::MissingCountdownTimestamp => {
            "Countdown timestamp mode requires a target Unix timestamp.".to_owned()
        }
        ValidationError::InvalidTimestamp(field) => {
            format!("Field '{field}' must be a valid non-zero Unix timestamp in seconds.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::model::ApplicationIdError;

    #[test]
    fn user_safe_message_never_leaks_os_paths_or_peer_messages() {
        let io_err = DiDiError::Storage(StorageError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            r"C:\Users\Alice\AppData\Roaming\DiDi\config.json",
        )));
        let msg = io_err.user_safe_message();
        assert!(!msg.contains("Alice"));
        assert!(!msg.contains("AppData"));

        let rpc_err = DiDiError::Discord(DiscordError::RpcRejected {
            code: 5000,
            message: "SECRET_PEER_DETAILS".to_owned(),
        });
        assert!(!rpc_err.user_safe_message().contains("SECRET_PEER_DETAILS"));

        let val_err = DiDiError::Validation(ValidationError::ApplicationId(
            ApplicationIdError::InvalidFormat,
        ));
        assert!(val_err.user_safe_message().contains("17 and 20"));
    }
}
