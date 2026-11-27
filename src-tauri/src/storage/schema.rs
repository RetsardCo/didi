use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub use crate::presence::model::TimestampMode;
use crate::presence::model::{
    ActivityButton, ActivityType, ArtworkRef, PresenceActivity, StatusDisplayType,
};
use crate::presence::validation::{
    validate_activity_name_override, validate_application_id, validate_artwork_ref,
    validate_buttons, validate_https_url, validate_profile_id, validate_profile_name,
    validate_text_field, validate_unix_timestamp_secs, ValidationError, MAX_PROFILES_COUNT,
    MAX_URL_LEN,
};
use crate::storage::errors::StorageError;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const CURRENT_BUNDLE_VERSION: u32 = 1;
pub const MAX_IMPORT_BYTES: usize = 262_144; // 256 KiB hard limit per ADR D-020 & Threat T-01

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiLayout {
    #[default]
    Compact,
    Expanded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileV1 {
    pub id: String,
    pub name: String,
    pub application_id_override: Option<String>,
    pub activity_name_override: Option<String>,
    pub activity_type: ActivityType,
    pub status_display_type: StatusDisplayType,
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
    pub timestamp_mode: TimestampMode,
    pub custom_timestamp_unix: Option<u64>,
    pub buttons: Vec<ActivityButton>,
}

impl ProfileV1 {
    #[must_use]
    pub fn default_profile() -> Self {
        Self {
            id: "default".to_owned(),
            name: "Default Profile".to_owned(),
            application_id_override: None,
            activity_name_override: None,
            activity_type: ActivityType::Playing,
            status_display_type: StatusDisplayType::Name,
            details: Some("Custom Rich Presence".to_owned()),
            details_url: None,
            state: Some("Active".to_owned()),
            state_url: None,
            large_asset: ArtworkRef::None,
            large_text: None,
            large_url: None,
            small_asset: ArtworkRef::None,
            small_text: None,
            small_url: None,
            timestamp_mode: TimestampMode::None,
            custom_timestamp_unix: None,
            buttons: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_profile_id(&self.id)?;
        validate_profile_name(&self.name)?;

        if let Some(ref app_id) = self.application_id_override {
            validate_application_id(app_id)?;
        }

        if let Some(ref act_name) = self.activity_name_override {
            validate_activity_name_override(act_name)?;
        }

        if let Some(ref det) = self.details {
            validate_text_field("details", det)?;
        }

        if let Some(ref url) = self.details_url {
            validate_https_url("details_url", url, MAX_URL_LEN)?;
        }

        if let Some(ref st) = self.state {
            validate_text_field("state", st)?;
        }

        if let Some(ref url) = self.state_url {
            validate_https_url("state_url", url, MAX_URL_LEN)?;
        }

        validate_artwork_ref("large_asset", &self.large_asset)?;
        if let Some(ref text) = self.large_text {
            validate_text_field("large_text", text)?;
        }
        if let Some(ref url) = self.large_url {
            validate_https_url("large_url", url, MAX_URL_LEN)?;
        }

        validate_artwork_ref("small_asset", &self.small_asset)?;
        if let Some(ref text) = self.small_text {
            validate_text_field("small_text", text)?;
        }
        if let Some(ref url) = self.small_url {
            validate_https_url("small_url", url, MAX_URL_LEN)?;
        }

        validate_buttons(&self.buttons)?;

        match self.timestamp_mode {
            TimestampMode::CountdownUntil => {
                let ts = self
                    .custom_timestamp_unix
                    .ok_or(ValidationError::MissingCountdownTimestamp)?;
                validate_unix_timestamp_secs("custom_timestamp_unix", ts)?;
            }
            TimestampMode::None | TimestampMode::SessionElapsed => {
                if let Some(ts) = self.custom_timestamp_unix {
                    validate_unix_timestamp_secs("custom_timestamp_unix", ts)?;
                }
            }
        }

        Ok(())
    }

    /// Rejects raw control characters before normalizing permitted whitespace, converts empty
    /// optional text fields to `None`, trims Application ID override, and auto-lowercases
    /// Discord Developer Portal asset keys per ADR D-015.
    pub fn normalize(&mut self) -> Result<(), ValidationError> {
        if self.id.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidControlChars("profile.id"));
        }
        self.id = self.id.trim().to_owned();

        if self.name.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidControlChars("profile.name"));
        }
        self.name = self.name.trim().to_owned();

        if let Some(ref app_id) = self.application_id_override {
            if app_id.chars().any(|c| c.is_control()) {
                return Err(ValidationError::InvalidControlChars(
                    "application_id_override",
                ));
            }
            let trimmed = app_id.trim();
            self.application_id_override = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            };
        }

        self.activity_name_override =
            normalize_optional_text("activity_name_override", self.activity_name_override.take())?;
        self.details = normalize_optional_text("details", self.details.take())?;
        self.details_url = normalize_optional_url(self.details_url.take());
        self.state = normalize_optional_text("state", self.state.take())?;
        self.state_url = normalize_optional_url(self.state_url.take());

        normalize_artwork_ref("large_asset", &mut self.large_asset)?;
        self.large_text = normalize_optional_text("large_text", self.large_text.take())?;
        self.large_url = normalize_optional_url(self.large_url.take());

        normalize_artwork_ref("small_asset", &mut self.small_asset)?;
        self.small_text = normalize_optional_text("small_text", self.small_text.take())?;
        self.small_url = normalize_optional_url(self.small_url.take());

        for button in &mut self.buttons {
            if button.label.chars().any(|c| c.is_control()) {
                return Err(ValidationError::InvalidControlChars("button.label"));
            }
            button.label = button.label.trim().to_owned();
        }

        Ok(())
    }

    pub fn normalize_and_validate(&mut self) -> Result<(), ValidationError> {
        self.normalize()?;
        self.validate()
    }

    #[must_use]
    pub fn to_presence_activity(&self, session_start_unix: Option<u64>) -> PresenceActivity {
        let (start_timestamp, end_timestamp) = match self.timestamp_mode {
            TimestampMode::None => (None, None),
            TimestampMode::SessionElapsed => (session_start_unix, None),
            TimestampMode::CountdownUntil => (None, self.custom_timestamp_unix),
        };

        self.build_presence_activity(start_timestamp, end_timestamp)
    }

    #[must_use]
    pub fn to_presence_activity_at(
        &self,
        session_start_unix: Option<u64>,
        now_unix: u64,
    ) -> PresenceActivity {
        let (start_timestamp, end_timestamp) = match self.timestamp_mode {
            TimestampMode::None => (None, None),
            TimestampMode::SessionElapsed => (session_start_unix, None),
            TimestampMode::CountdownUntil => {
                (None, self.custom_timestamp_unix.filter(|&ts| ts > now_unix))
            }
        };

        self.build_presence_activity(start_timestamp, end_timestamp)
    }

    #[must_use]
    fn build_presence_activity(
        &self,
        start_timestamp: Option<u64>,
        end_timestamp: Option<u64>,
    ) -> PresenceActivity {
        PresenceActivity {
            activity_type: self.activity_type,
            status_display_type: Some(self.status_display_type),
            name_override: self.activity_name_override.clone(),
            details: self.details.clone(),
            details_url: self.details_url.clone(),
            state: self.state.clone(),
            state_url: self.state_url.clone(),
            large_asset: self.large_asset.clone(),
            large_text: self.large_text.clone(),
            large_url: self.large_url.clone(),
            small_asset: self.small_asset.clone(),
            small_text: self.small_text.clone(),
            small_url: self.small_url.clone(),
            timestamp_mode: self.timestamp_mode,
            start_timestamp,
            end_timestamp,
            buttons: self.buttons.clone(),
        }
    }

    #[must_use]
    pub fn effective_application_id<'a>(&'a self, default_id: Option<&'a str>) -> Option<&'a str> {
        self.application_id_override.as_deref().or(default_id)
    }
}

fn normalize_optional_text(
    field_name: &'static str,
    value: Option<String>,
) -> Result<Option<String>, ValidationError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.chars().any(|c| c.is_control()) {
        return Err(ValidationError::InvalidControlChars(field_name));
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

fn normalize_optional_url(value: Option<String>) -> Option<String> {
    match value {
        Some(raw) if raw.is_empty() => None,
        other => other,
    }
}

fn normalize_artwork_ref(
    field_name: &'static str,
    asset: &mut ArtworkRef,
) -> Result<(), ValidationError> {
    if let ArtworkRef::PortalKey { key } = asset {
        if key.chars().any(|c| c.is_control()) {
            return Err(ValidationError::InvalidControlChars(field_name));
        }
        *key = key.trim().to_ascii_lowercase();
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfigV1 {
    pub schema_version: u32,
    pub default_application_id: Option<String>,
    pub active_profile_id: String,
    pub presence_enabled: bool,
    pub launch_at_login: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub ui_layout: UiLayout,
    pub profiles: Vec<ProfileV1>,
}

impl Default for AppConfigV1 {
    fn default() -> Self {
        Self::default_config()
    }
}

impl AppConfigV1 {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            default_application_id: None,
            active_profile_id: "default".to_owned(),
            presence_enabled: true,
            launch_at_login: false,
            start_minimized: true,
            close_to_tray: true,
            ui_layout: UiLayout::Compact,
            profiles: vec![ProfileV1::default_profile()],
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(ref app_id) = self.default_application_id {
            validate_application_id(app_id)?;
        }

        if self.profiles.is_empty() {
            return Err(ValidationError::NoProfiles);
        }

        if self.profiles.len() > MAX_PROFILES_COUNT {
            return Err(ValidationError::TooManyProfiles {
                max: MAX_PROFILES_COUNT,
                actual: self.profiles.len(),
            });
        }

        let mut seen_ids = HashSet::new();
        let mut active_found = false;

        for profile in &self.profiles {
            if !seen_ids.insert(&profile.id) {
                return Err(ValidationError::DuplicateProfileId(profile.id.clone()));
            }
            if profile.id == self.active_profile_id {
                active_found = true;
            }
            profile.validate()?;
        }

        if !active_found {
            return Err(ValidationError::ActiveProfileNotFound(
                self.active_profile_id.clone(),
            ));
        }

        Ok(())
    }

    pub fn active_profile(&self) -> Result<&ProfileV1, StorageError> {
        self.profiles
            .iter()
            .find(|p| p.id == self.active_profile_id)
            .ok_or_else(|| StorageError::ProfileNotFound(self.active_profile_id.clone()))
    }

    #[must_use]
    pub fn find_profile(&self, id: &str) -> Option<&ProfileV1> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn find_profile_mut(&mut self, id: &str) -> Option<&mut ProfileV1> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }
}

/// Credential-free profile representation for export and import (ADR D-020, Section 4.5).
///
/// Explicitly excludes `default_application_id`, `application_id_override`,
/// local file paths, and preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportProfileV1 {
    pub id: String,
    pub name: String,
    pub activity_name_override: Option<String>,
    pub activity_type: ActivityType,
    pub status_display_type: StatusDisplayType,
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
    pub timestamp_mode: TimestampMode,
    pub custom_timestamp_unix: Option<u64>,
    pub buttons: Vec<ActivityButton>,
}

impl From<&ProfileV1> for ExportProfileV1 {
    fn from(p: &ProfileV1) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            activity_name_override: p.activity_name_override.clone(),
            activity_type: p.activity_type,
            status_display_type: p.status_display_type,
            details: p.details.clone(),
            details_url: p.details_url.clone(),
            state: p.state.clone(),
            state_url: p.state_url.clone(),
            large_asset: p.large_asset.clone(),
            large_text: p.large_text.clone(),
            large_url: p.large_url.clone(),
            small_asset: p.small_asset.clone(),
            small_text: p.small_text.clone(),
            small_url: p.small_url.clone(),
            timestamp_mode: p.timestamp_mode,
            custom_timestamp_unix: p.custom_timestamp_unix,
            buttons: p.buttons.clone(),
        }
    }
}

impl From<ExportProfileV1> for ProfileV1 {
    fn from(ep: ExportProfileV1) -> Self {
        Self {
            id: ep.id,
            name: ep.name,
            application_id_override: None, // Credentials strictly stripped on import
            activity_name_override: ep.activity_name_override,
            activity_type: ep.activity_type,
            status_display_type: ep.status_display_type,
            details: ep.details,
            details_url: ep.details_url,
            state: ep.state,
            state_url: ep.state_url,
            large_asset: ep.large_asset,
            large_text: ep.large_text,
            large_url: ep.large_url,
            small_asset: ep.small_asset,
            small_text: ep.small_text,
            small_url: ep.small_url,
            timestamp_mode: ep.timestamp_mode,
            custom_timestamp_unix: ep.custom_timestamp_unix,
            buttons: ep.buttons,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportBundleV1 {
    pub bundle_version: u32,
    pub exported_at_unix: u64,
    pub profiles: Vec<ExportProfileV1>,
}

impl ExportBundleV1 {
    #[must_use]
    pub fn from_profiles(profiles: &[ProfileV1], exported_at_unix: u64) -> Self {
        Self {
            bundle_version: CURRENT_BUNDLE_VERSION,
            exported_at_unix,
            profiles: profiles.iter().map(ExportProfileV1::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportConflictStrategy {
    Overwrite,
    KeepBoth,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConflict {
    pub existing_id: String,
    pub existing_name: String,
    pub imported_id: String,
    pub imported_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub activity_type: ActivityType,
    pub details: Option<String>,
    pub state: Option<String>,
}

impl From<&ProfileV1> for ProfileSummary {
    fn from(p: &ProfileV1) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            activity_type: p.activity_type,
            details: p.details.clone(),
            state: p.state.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    pub bundle_version: u32,
    pub total_profiles: usize,
    pub new_profiles: Vec<ProfileSummary>,
    pub conflicting_profiles: Vec<ProfileConflict>,
    pub would_exceed_max_profiles: bool,
    pub would_exceed_max_profiles_keep_both: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates_cleanly() {
        let config = AppConfigV1::default_config();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.active_profile_id, "default");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_rejects_missing_active_profile() {
        let mut config = AppConfigV1::default_config();
        config.active_profile_id = "nonexistent".to_owned();
        assert_eq!(
            config.validate(),
            Err(ValidationError::ActiveProfileNotFound(
                "nonexistent".to_owned()
            ))
        );
    }

    #[test]
    fn config_rejects_duplicate_profile_ids() {
        let mut config = AppConfigV1::default_config();
        let mut duplicate = ProfileV1::default_profile();
        duplicate.name = "Duplicate".to_owned();
        config.profiles.push(duplicate);
        assert_eq!(
            config.validate(),
            Err(ValidationError::DuplicateProfileId("default".to_owned()))
        );
    }

    #[test]
    fn export_bundle_strips_application_id_and_deserializes_cleanly(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut profile = ProfileV1::default_profile();
        profile.application_id_override = Some("123456789012345678".to_owned());

        let bundle = ExportBundleV1::from_profiles(&[profile], 1700000000);
        assert_eq!(bundle.bundle_version, 1);
        assert_eq!(bundle.profiles.len(), 1);

        let json = serde_json::to_string_pretty(&bundle)?;
        assert!(!json.contains("123456789012345678"));
        assert!(!json.contains("application_id_override"));

        let deserialized: ExportBundleV1 = serde_json::from_str(&json)?;
        let imported_profile: ProfileV1 = deserialized.profiles[0].clone().into();
        assert_eq!(imported_profile.application_id_override, None);
        assert!(imported_profile.validate().is_ok());
        Ok(())
    }

    #[test]
    fn export_bundle_rejects_unknown_fields_in_json() {
        let json_with_injected_cmd = r#"{
            "bundle_version": 1,
            "exported_at_unix": 1700000000,
            "injected_command": "calc.exe",
            "profiles": []
        }"#;
        let res: Result<ExportBundleV1, _> = serde_json::from_str(json_with_injected_cmd);
        assert!(res.is_err());
    }

    #[test]
    fn to_presence_activity_maps_timestamp_modes() {
        let mut profile = ProfileV1::default_profile();
        profile.timestamp_mode = TimestampMode::None;
        let act = profile.to_presence_activity(Some(1000));
        assert_eq!(act.start_timestamp, None);
        assert_eq!(act.end_timestamp, None);

        profile.timestamp_mode = TimestampMode::SessionElapsed;
        let act = profile.to_presence_activity(Some(1000));
        assert_eq!(act.start_timestamp, Some(1000));
        assert_eq!(act.end_timestamp, None);

        profile.timestamp_mode = TimestampMode::CountdownUntil;
        profile.custom_timestamp_unix = Some(2000);
        let act = profile.to_presence_activity(Some(1000));
        assert_eq!(act.start_timestamp, None);
        assert_eq!(act.end_timestamp, Some(2000));

        // Expired countdown must be omitted when evaluated against current Unix time
        let expired_act = profile.to_presence_activity_at(Some(1000), 2000);
        assert_eq!(expired_act.end_timestamp, None);
        let expired_past = profile.to_presence_activity_at(Some(1000), 2500);
        assert_eq!(expired_past.end_timestamp, None);
        // Future countdown must be preserved
        let future_act = profile.to_presence_activity_at(Some(1000), 1500);
        assert_eq!(future_act.end_timestamp, Some(2000));
    }

    #[test]
    fn countdown_mode_requires_valid_timestamp_in_seconds() {
        let mut profile = ProfileV1::default_profile();
        profile.timestamp_mode = TimestampMode::CountdownUntil;
        profile.custom_timestamp_unix = None;
        assert_eq!(
            profile.validate(),
            Err(ValidationError::MissingCountdownTimestamp)
        );

        profile.custom_timestamp_unix = Some(0);
        assert_eq!(
            profile.validate(),
            Err(ValidationError::InvalidTimestamp("custom_timestamp_unix"))
        );

        // Millisecond-scale timestamp must be rejected
        profile.custom_timestamp_unix = Some(1_700_000_000_000);
        assert_eq!(
            profile.validate(),
            Err(ValidationError::InvalidTimestamp("custom_timestamp_unix"))
        );

        profile.custom_timestamp_unix = Some(1_900_000_000);
        assert!(profile.validate().is_ok());
    }

    #[test]
    fn normalize_trims_spaces_lowercases_portal_key_and_rejects_control_chars(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut profile = ProfileV1::default_profile();
        profile.id = "  my_profile  ".to_owned();
        profile.name = "  My Profile  ".to_owned();
        profile.application_id_override = Some("  123456789012345678  ".to_owned());
        profile.details = Some("   ".to_owned());
        profile.state = Some("  Working  ".to_owned());
        profile.large_asset = ArtworkRef::PortalKey {
            key: "  My_Portal_Key  ".to_owned(),
        };

        profile.normalize_and_validate()?;
        assert_eq!(profile.id, "my_profile");
        assert_eq!(profile.name, "My Profile");
        assert_eq!(
            profile.application_id_override.as_deref(),
            Some("123456789012345678")
        );
        assert_eq!(profile.details, None);
        assert_eq!(profile.state.as_deref(), Some("Working"));
        assert_eq!(
            profile.large_asset,
            ArtworkRef::PortalKey {
                key: "my_portal_key".to_owned()
            }
        );

        let mut bad_profile = ProfileV1::default_profile();
        bad_profile.details = Some("Hello\n".to_owned());
        assert_eq!(
            bad_profile.normalize_and_validate(),
            Err(ValidationError::InvalidControlChars("details"))
        );
        Ok(())
    }
}
