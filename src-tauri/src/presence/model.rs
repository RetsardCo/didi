use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const APP_ID_MIN_LEN: usize = 17;
const APP_ID_MAX_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplicationIdError {
    #[error("application ID must not be empty")]
    Empty,
    #[error("application ID must be between 17 and 20 ASCII digits")]
    InvalidFormat,
    #[error("application ID must be a non-zero Discord snowflake")]
    ZeroValue,
}

/// User-supplied Discord Application ID (BYO-AppID per D-013).
///
/// Masks all but the final 4 digits in `Debug` and `Display` output so accidental
/// formatting cannot leak the full credential into logs or diagnostics.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ApplicationId(String);

impl ApplicationId {
    pub fn parse(raw: &str) -> Result<Self, ApplicationIdError> {
        if raw.chars().any(|c| c.is_control()) {
            return Err(ApplicationIdError::InvalidFormat);
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ApplicationIdError::Empty);
        }
        if trimmed.len() < APP_ID_MIN_LEN
            || trimmed.len() > APP_ID_MAX_LEN
            || !trimmed.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(ApplicationIdError::InvalidFormat);
        }
        let parsed = trimmed
            .parse::<u64>()
            .map_err(|_| ApplicationIdError::InvalidFormat)?;
        if parsed == 0 {
            return Err(ApplicationIdError::ZeroValue);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn masked(&self) -> String {
        let suffix_start = self.0.len().saturating_sub(4);
        format!("****{}", &self.0[suffix_start..])
    }
}

impl fmt::Display for ApplicationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.masked())
    }
}

impl fmt::Debug for ApplicationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApplicationId({})", self.masked())
    }
}

/// Activity types officially documented for Discord `SET_ACTIVITY` (0, 2, 3, 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    Playing = 0,
    Listening = 2,
    Watching = 3,
    Competing = 5,
}

impl ActivityType {
    #[must_use]
    pub const fn wire_code(self) -> u8 {
        self as u8
    }
}

/// Controls which field displays in the Discord member list status text (Phase 3 verified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusDisplayType {
    Name = 0,
    State = 1,
    Details = 2,
}

impl StatusDisplayType {
    #[must_use]
    pub const fn wire_code(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtworkRef {
    None,
    PortalKey { key: String },
    ExternalUrl { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityButton {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampMode {
    #[default]
    None,
    SessionElapsed,
    CountdownUntil,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceActivity {
    pub activity_type: ActivityType,
    pub status_display_type: Option<StatusDisplayType>,
    pub name_override: Option<String>,
    pub details: Option<String>,
    pub details_url: Option<String>,
    pub state: Option<String>,
    pub state_url: Option<String>,
    pub large_asset: ArtworkRef,
    pub large_text: Option<String>,
    pub large_url: Option<String>,
    pub small_asset: ArtworkRef,
    pub small_text: Option<String>,
    pub small_url: Option<String>,
    #[serde(default)]
    pub timestamp_mode: TimestampMode,
    pub start_timestamp: Option<u64>,
    pub end_timestamp: Option<u64>,
    pub buttons: Vec<ActivityButton>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_id_accepts_valid_snowflake_and_masks_output(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let id = ApplicationId::parse("  123456789012345678  ")?;
        assert_eq!(id.as_str(), "123456789012345678");
        assert_eq!(id.masked(), "****5678");
        assert_eq!(format!("{id}"), "****5678");
        assert_eq!(format!("{id:?}"), "ApplicationId(****5678)");
        assert!(!format!("{id:?}").contains("1234567890"));
        Ok(())
    }

    #[test]
    fn application_id_rejects_invalid_inputs() {
        assert_eq!(ApplicationId::parse(""), Err(ApplicationIdError::Empty));
        assert_eq!(ApplicationId::parse("   "), Err(ApplicationIdError::Empty));
        assert_eq!(
            ApplicationId::parse("1234567890123456"),
            Err(ApplicationIdError::InvalidFormat)
        );
        assert_eq!(
            ApplicationId::parse("123456789012345678901"),
            Err(ApplicationIdError::InvalidFormat)
        );
        assert_eq!(
            ApplicationId::parse("12345678901234567a"),
            Err(ApplicationIdError::InvalidFormat)
        );
        assert_eq!(
            ApplicationId::parse("000000000000000000"),
            Err(ApplicationIdError::ZeroValue)
        );
    }

    #[test]
    fn activity_type_wire_codes_match_discord_spec() {
        assert_eq!(ActivityType::Playing.wire_code(), 0);
        assert_eq!(ActivityType::Listening.wire_code(), 2);
        assert_eq!(ActivityType::Watching.wire_code(), 3);
        assert_eq!(ActivityType::Competing.wire_code(), 5);
    }
}
