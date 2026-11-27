use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use thiserror::Error;

use crate::presence::model::{ActivityButton, ApplicationId, ApplicationIdError, ArtworkRef};

pub const MAX_PROFILE_NAME_LEN: usize = 64;
pub const MAX_ACTIVITY_NAME_LEN: usize = 128;
pub const MIN_TEXT_LEN: usize = 2;
pub const MAX_TEXT_LEN: usize = 128;
pub const MAX_URL_LEN: usize = 256;
pub const MAX_BUTTON_URL_LEN: usize = 512;
pub const MAX_BUTTON_LABEL_LEN: usize = 32;
pub const MAX_PORTAL_KEY_LEN: usize = 300;
pub const MAX_BUTTONS_COUNT: usize = 2;
pub const MAX_PROFILES_COUNT: usize = 50;
pub const MAX_PROFILE_ID_LEN: usize = 64;
/// Upper bound on Unix seconds timestamps (~year 3000), preventing accidental millisecond timestamps.
pub const MAX_UNIX_TIMESTAMP_SECS: u64 = 32_503_680_000;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("application ID error: {0}")]
    ApplicationId(#[from] ApplicationIdError),

    #[error("field '{0}' must not be empty")]
    EmptyField(&'static str),

    #[error("field '{field}' length {actual} is outside allowed range {min}..={max}")]
    LengthOutOfRange {
        field: &'static str,
        min: usize,
        max: usize,
        actual: usize,
    },

    #[error("field '{0}' contains prohibited control characters or newlines")]
    InvalidControlChars(&'static str),

    #[error("URL in field '{0}' must strictly use 'https://' scheme")]
    InvalidUrlScheme(&'static str),

    #[error("URL in field '{0}' contains prohibited credentials")]
    InvalidUrlCredentials(&'static str),

    #[error("URL in field '{0}' targets disallowed host (private/loopback or invalid)")]
    DisallowedUrlHost(&'static str),

    #[error("portal key '{0}' must be 1..=300 lowercase ASCII alphanumeric, '-', or '_' chars")]
    InvalidPortalKey(String),

    #[error("too many buttons: maximum is {max}, found {actual}")]
    TooManyButtons { max: usize, actual: usize },

    #[error("too many profiles: maximum is {max}, found {actual}")]
    TooManyProfiles { max: usize, actual: usize },

    #[error("profiles list cannot be empty")]
    NoProfiles,

    #[error("duplicate profile ID: '{0}'")]
    DuplicateProfileId(String),

    #[error("active profile ID '{0}' does not match any profile in the configuration")]
    ActiveProfileNotFound(String),

    #[error("profile ID '{0}' must be 1..=64 lowercase ASCII alphanumeric, '-', or '_' chars")]
    InvalidProfileIdFormat(String),

    #[error("countdown timestamp mode requires a target Unix timestamp")]
    MissingCountdownTimestamp,

    #[error("field '{0}' must be a valid non-zero Unix timestamp in seconds")]
    InvalidTimestamp(&'static str),
}

/// Validates a user-supplied Application ID if present.
pub fn validate_application_id(raw: &str) -> Result<ApplicationId, ValidationError> {
    ApplicationId::parse(raw).map_err(ValidationError::ApplicationId)
}

/// Validates profile ID format: `[a-z0-9_-]{1..=64}` with no raw control characters.
pub fn validate_profile_id(id: &str) -> Result<(), ValidationError> {
    if id.chars().any(|c| c.is_control()) {
        return Err(ValidationError::InvalidControlChars("profile.id"));
    }
    let trimmed = id.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_PROFILE_ID_LEN {
        return Err(ValidationError::InvalidProfileIdFormat(id.to_owned()));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(ValidationError::InvalidProfileIdFormat(id.to_owned()));
    }
    Ok(())
}

/// Validates profile human-readable name: `1..=64` chars, trimmed non-empty, no raw control chars.
pub fn validate_profile_name(name: &str) -> Result<(), ValidationError> {
    if name.chars().any(|c| c.is_control()) {
        return Err(ValidationError::InvalidControlChars("profile.name"));
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::EmptyField("profile.name"));
    }
    let char_count = trimmed.chars().count();
    if char_count > MAX_PROFILE_NAME_LEN {
        return Err(ValidationError::LengthOutOfRange {
            field: "profile.name",
            min: 1,
            max: MAX_PROFILE_NAME_LEN,
            actual: char_count,
        });
    }
    Ok(())
}

/// Validates optional activity name override: `2..=128` chars, single line, no raw control chars.
pub fn validate_activity_name_override(name: &str) -> Result<(), ValidationError> {
    validate_text_field("activity_name_override", name)
}

/// Validates single-line text fields (details, state, large_text, small_text): `2..=128` chars.
pub fn validate_text_field(field_name: &'static str, text: &str) -> Result<(), ValidationError> {
    if text.chars().any(|c| c.is_control()) {
        return Err(ValidationError::InvalidControlChars(field_name));
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::EmptyField(field_name));
    }
    let char_count = trimmed.chars().count();
    if !(MIN_TEXT_LEN..=MAX_TEXT_LEN).contains(&char_count) {
        return Err(ValidationError::LengthOutOfRange {
            field: field_name,
            min: MIN_TEXT_LEN,
            max: MAX_TEXT_LEN,
            actual: char_count,
        });
    }
    Ok(())
}

/// Validates a Unix timestamp in seconds (non-zero, bounded below millisecond scale).
pub fn validate_unix_timestamp_secs(
    field_name: &'static str,
    timestamp_unix: u64,
) -> Result<(), ValidationError> {
    if timestamp_unix == 0 || timestamp_unix > MAX_UNIX_TIMESTAMP_SECS {
        return Err(ValidationError::InvalidTimestamp(field_name));
    }
    Ok(())
}

/// Validates strict HTTPS URL per ADR D-021:
/// - starts with `https://`
/// - length <= `max_len`
/// - no whitespace, no ASCII control chars
/// - no embedded credentials (`user:pass@`)
/// - non-empty host, not loopback or RFC1918 private IP, not localhost
pub fn validate_https_url(
    field_name: &'static str,
    url: &str,
    max_len: usize,
) -> Result<(), ValidationError> {
    if url.is_empty() {
        return Err(ValidationError::EmptyField(field_name));
    }

    if url.chars().any(|c| c.is_control()) || url.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(ValidationError::InvalidControlChars(field_name));
    }

    let trimmed = url.trim();
    if trimmed.chars().count() > max_len {
        return Err(ValidationError::LengthOutOfRange {
            field: field_name,
            min: 8,
            max: max_len,
            actual: trimmed.chars().count(),
        });
    }

    if trimmed.len() < 8 || !trimmed[..8].eq_ignore_ascii_case("https://") {
        return Err(ValidationError::InvalidUrlScheme(field_name));
    }

    if trimmed.contains('\\') {
        return Err(ValidationError::InvalidControlChars(field_name));
    }

    let remainder = &trimmed[8..];
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];

    if authority.contains('@') {
        return Err(ValidationError::InvalidUrlCredentials(field_name));
    }

    let host_part = if authority.starts_with('[') {
        // IPv6 literal: [::1]:443
        if let Some(bracket_end) = authority.find(']') {
            let inner_ip = &authority[1..bracket_end];
            let after_bracket = &authority[bracket_end + 1..];
            if !after_bracket.is_empty() {
                if !after_bracket.starts_with(':') {
                    return Err(ValidationError::DisallowedUrlHost(field_name));
                }
                let port_str = &after_bracket[1..];
                if !is_valid_port(port_str) {
                    return Err(ValidationError::DisallowedUrlHost(field_name));
                }
            }
            inner_ip
        } else {
            return Err(ValidationError::DisallowedUrlHost(field_name));
        }
    } else if let Some(colon_pos) = authority.find(':') {
        let host = &authority[..colon_pos];
        let port_str = &authority[colon_pos + 1..];
        if !is_valid_port(port_str) {
            return Err(ValidationError::DisallowedUrlHost(field_name));
        }
        host
    } else {
        authority
    };

    let host_lower = host_part.to_ascii_lowercase();
    if host_lower.is_empty() {
        return Err(ValidationError::DisallowedUrlHost(field_name));
    }

    // Check if host is IPv4 literal
    if let Ok(ipv4) = Ipv4Addr::from_str(&host_lower) {
        // Disallow leading zeros in octet strings to prevent octal loopback ambiguity (e.g. 0177.0.0.1)
        for part in host_lower.split('.') {
            if part.len() > 1 && part.starts_with('0') {
                return Err(ValidationError::DisallowedUrlHost(field_name));
            }
        }
        if is_disallowed_ipv4(ipv4) {
            return Err(ValidationError::DisallowedUrlHost(field_name));
        }
        return Ok(());
    }

    // Check if host is IPv6 literal
    if let Ok(ipv6) = Ipv6Addr::from_str(&host_lower) {
        if is_disallowed_ipv6(ipv6) {
            return Err(ValidationError::DisallowedUrlHost(field_name));
        }
        return Ok(());
    }

    // Named domain checks
    if host_lower == "localhost"
        || host_lower.ends_with(".localhost")
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".lan")
    {
        return Err(ValidationError::DisallowedUrlHost(field_name));
    }

    // Validate domain characters: at least 2 labels separated by dots (prohibiting unqualified local hostnames)
    let labels: Vec<&str> = host_lower.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| label.is_empty()) {
        return Err(ValidationError::DisallowedUrlHost(field_name));
    }

    // Top-level domain (last label) cannot be all-numeric (RFC 3696)
    if labels
        .last()
        .is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ValidationError::DisallowedUrlHost(field_name));
    }

    for label in &labels {
        if label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ValidationError::DisallowedUrlHost(field_name));
        }
    }

    Ok(())
}

fn is_valid_port(port_str: &str) -> bool {
    !port_str.is_empty()
        && port_str.bytes().all(|b| b.is_ascii_digit())
        && port_str.parse::<u16>().is_ok_and(|port| port > 0)
}

fn is_disallowed_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 0.0.0.0/8 (unspecified)
    if octets[0] == 0 {
        return true;
    }
    // 127.0.0.0/8 (loopback)
    if octets[0] == 127 || ip.is_loopback() {
        return true;
    }
    // 10.0.0.0/8 (private)
    if octets[0] == 10 {
        return true;
    }
    // 100.64.0.0/10 (carrier-grade NAT / shared)
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    // 172.16.0.0/12 (private: 172.16.x.x - 172.31.x.x)
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    // 192.0.0.0/24 (IETF protocol assignments)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return true;
    }
    // 192.0.2.0/24 (TEST-NET-1 / documentation)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
        return true;
    }
    // 192.168.0.0/16 (private)
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // 169.254.0.0/16 (link-local)
    if octets[0] == 169 && octets[1] == 254 || ip.is_link_local() {
        return true;
    }
    // 198.51.100.0/24 (TEST-NET-2)
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
        return true;
    }
    // 203.0.113.0/24 (TEST-NET-3)
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
        return true;
    }
    // 224.0.0.0/4 (multicast: 224..=239)
    if (224..=239).contains(&octets[0]) || ip.is_multicast() {
        return true;
    }
    // 240.0.0.0/4 (reserved / broadcast)
    if octets[0] >= 240 || ip.is_broadcast() {
        return true;
    }
    false
}

fn is_disallowed_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    let segments = ip.segments();
    // fe80::/10 link-local
    if (segments[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // fc00::/7 unique local
    if (segments[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // 2001:db8::/32 documentation
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return true;
    }
    // 100::/64 discard
    if segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0 {
        return true;
    }
    // IPv4-mapped (::ffff:a.b.c.d) or IPv4-compatible (::a.b.c.d)
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && (segments[5] == 0 || segments[5] == 0xffff)
    {
        let ipv4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xff) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xff) as u8,
        );
        if is_disallowed_ipv4(ipv4) {
            return true;
        }
    }
    // NAT64 local-use translation prefix (64:ff9b:1::/48)
    if segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001 {
        return true;
    }
    // NAT64 well-known prefix (64:ff9b::/96) embedding IPv4 in segments[6..8]
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
        && segments[5] == 0
    {
        let ipv4 = Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            (segments[6] & 0xff) as u8,
            (segments[7] >> 8) as u8,
            (segments[7] & 0xff) as u8,
        );
        if is_disallowed_ipv4(ipv4) {
            return true;
        }
    }
    // 6to4 (2002::/16) embedding IPv4 in segments[1..3]
    if segments[0] == 0x2002 {
        let ipv4 = Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            (segments[1] & 0xff) as u8,
            (segments[2] >> 8) as u8,
            (segments[2] & 0xff) as u8,
        );
        if is_disallowed_ipv4(ipv4) {
            return true;
        }
    }
    // Teredo tunneling (2001:0000::/32)
    if segments[0] == 0x2001 && segments[1] == 0x0000 {
        return true;
    }
    false
}

/// Validates ArtworkRef: portal key or external URL.
pub fn validate_artwork_ref(
    field_name: &'static str,
    asset: &ArtworkRef,
) -> Result<(), ValidationError> {
    match asset {
        ArtworkRef::None => Ok(()),
        ArtworkRef::PortalKey { key } => {
            if key.chars().any(|c| c.is_control()) {
                return Err(ValidationError::InvalidControlChars(field_name));
            }
            let trimmed = key.trim();
            if trimmed.is_empty() || trimmed.len() > MAX_PORTAL_KEY_LEN {
                return Err(ValidationError::InvalidPortalKey(key.clone()));
            }
            if !trimmed
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
            {
                return Err(ValidationError::InvalidPortalKey(key.clone()));
            }
            Ok(())
        }
        ArtworkRef::ExternalUrl { url } => validate_https_url(field_name, url, MAX_PORTAL_KEY_LEN),
    }
}

/// Validates an individual ActivityButton.
pub fn validate_button(button: &ActivityButton) -> Result<(), ValidationError> {
    if button.label.chars().any(|c| c.is_control()) {
        return Err(ValidationError::InvalidControlChars("button.label"));
    }
    let label_trimmed = button.label.trim();
    if label_trimmed.is_empty() {
        return Err(ValidationError::EmptyField("button.label"));
    }
    let char_count = label_trimmed.chars().count();
    if char_count > MAX_BUTTON_LABEL_LEN {
        return Err(ValidationError::LengthOutOfRange {
            field: "button.label",
            min: 1,
            max: MAX_BUTTON_LABEL_LEN,
            actual: char_count,
        });
    }

    validate_https_url("button.url", &button.url, MAX_BUTTON_URL_LEN)
}

/// Validates buttons collection: `0..=2` items, each valid.
pub fn validate_buttons(buttons: &[ActivityButton]) -> Result<(), ValidationError> {
    if buttons.len() > MAX_BUTTONS_COUNT {
        return Err(ValidationError::TooManyButtons {
            max: MAX_BUTTONS_COUNT,
            actual: buttons.len(),
        });
    }
    for button in buttons {
        validate_button(button)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_profile_id_rules() {
        assert!(validate_profile_id("default").is_ok());
        assert!(validate_profile_id("work_profile-2").is_ok());
        assert!(validate_profile_id("").is_err());
        assert!(validate_profile_id("HasUpperCase").is_err());
        assert!(validate_profile_id("invalid space").is_err());
        assert!(validate_profile_id("invalid/slash").is_err());
        let oversized = "a".repeat(65);
        assert!(validate_profile_id(&oversized).is_err());
    }

    #[test]
    fn validates_profile_name_rules() {
        assert!(validate_profile_name("Gaming").is_ok());
        assert!(validate_profile_name("").is_err());
        assert!(validate_profile_name("   ").is_err());
        assert!(validate_profile_name("Has\nNewline").is_err());
        let oversized = "a".repeat(65);
        assert!(validate_profile_name(&oversized).is_err());
    }

    #[test]
    fn validates_activity_name_override_rules() {
        assert!(validate_activity_name_override("Custom Game").is_ok());
        assert!(validate_activity_name_override("A").is_err());
        assert!(validate_activity_name_override("").is_err());
        assert!(validate_activity_name_override("Has\rReturn").is_err());
        let oversized = "a".repeat(129);
        assert!(validate_activity_name_override(&oversized).is_err());
    }

    #[test]
    fn validates_text_field_rules() {
        assert!(validate_text_field("details", "Level 42").is_ok());
        assert!(validate_text_field("details", "L").is_err());
        assert!(validate_text_field("details", "").is_err());
        assert!(validate_text_field("details", "Has\tTab").is_err());
        let oversized = "a".repeat(129);
        assert!(validate_text_field("details", &oversized).is_err());
    }

    #[test]
    fn validates_https_url_accepts_valid_urls() {
        assert!(
            validate_https_url("details_url", "https://github.com/RetsardCo/didi", 256).is_ok()
        );
        assert!(validate_https_url("button.url", "https://discord.gg/example", 512).is_ok());
        assert!(
            validate_https_url("asset_url", "https://cdn.example.org/images/logo.png", 300).is_ok()
        );
        assert!(validate_https_url("port_url", "https://example.com:8443/status", 256).is_ok());
        assert!(validate_https_url("mixed_case", "Https://example.com/mixed", 256).is_ok());
        assert!(validate_https_url("upper_scheme", "HTTPS://example.com/upper", 256).is_ok());
    }

    #[test]
    fn validates_https_url_rejects_insecure_or_disallowed_schemes() {
        assert_eq!(
            validate_https_url("url", "http://example.com", 256),
            Err(ValidationError::InvalidUrlScheme("url"))
        );
        assert_eq!(
            validate_https_url("url", "javascript:alert(1)", 256),
            Err(ValidationError::InvalidUrlScheme("url"))
        );
        assert_eq!(
            validate_https_url("url", "file:///C:/passwords.txt", 256),
            Err(ValidationError::InvalidUrlScheme("url"))
        );
        assert_eq!(
            validate_https_url("url", "data:text/plain,hello", 256),
            Err(ValidationError::InvalidUrlScheme("url"))
        );
    }

    #[test]
    fn validates_https_url_rejects_credentials_and_control_chars() {
        assert_eq!(
            validate_https_url("url", "https://admin:secret@example.com/api", 256),
            Err(ValidationError::InvalidUrlCredentials("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://example.com/api\n", 256),
            Err(ValidationError::InvalidControlChars("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://example.com\\path", 256),
            Err(ValidationError::InvalidControlChars("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://example.com/path\u{0085}", 256),
            Err(ValidationError::InvalidControlChars("url"))
        );
    }

    #[test]
    fn validates_https_url_rejects_private_and_loopback_hosts() {
        assert_eq!(
            validate_https_url("url", "https://127.0.0.1/test", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://localhost/test", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://192.168.1.100/admin", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://10.0.0.1/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://172.16.0.1/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://169.254.1.1/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://server.local/api", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // IPv4-mapped IPv6 literals
        assert_eq!(
            validate_https_url("url", "https://[::ffff:127.0.0.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[::ffff:192.168.1.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // IPv4-compatible IPv6 literal
        assert_eq!(
            validate_https_url("url", "https://[::127.0.0.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // IPv6 documentation & multicast
        assert_eq!(
            validate_https_url("url", "https://[2001:db8::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[ff02::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // Invalid IPv6 port
        assert_eq!(
            validate_https_url("url", "https://[2001:db8::1]:invalid_port/path", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // Octal leading zero and decimal integer IP
        assert_eq!(
            validate_https_url("url", "https://0177.0.0.1/test", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://2130706433/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // Unqualified single-word hostnames
        assert_eq!(
            validate_https_url("url", "https://router/admin", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://intranet/wiki", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
    }

    #[test]
    fn validates_portal_key_rules() {
        assert!(validate_artwork_ref("large_asset", &ArtworkRef::None).is_ok());
        assert!(validate_artwork_ref(
            "large_asset",
            &ArtworkRef::PortalKey {
                key: "valid_key-123".to_owned()
            }
        )
        .is_ok());
        assert!(validate_artwork_ref(
            "large_asset",
            &ArtworkRef::PortalKey {
                key: "INVALID_UPPERCASE".to_owned()
            }
        )
        .is_err());
        assert!(
            validate_artwork_ref("large_asset", &ArtworkRef::PortalKey { key: "".to_owned() })
                .is_err()
        );
        assert!(validate_artwork_ref(
            "large_asset",
            &ArtworkRef::PortalKey {
                key: "has space".to_owned()
            }
        )
        .is_err());
    }

    #[test]
    fn validates_buttons_rules() {
        let valid_button = ActivityButton {
            label: "Visit Repo".to_owned(),
            url: "https://github.com/RetsardCo/didi".to_owned(),
        };
        assert!(validate_buttons(&[]).is_ok());
        assert!(validate_buttons(std::slice::from_ref(&valid_button)).is_ok());
        assert!(validate_buttons(&[valid_button.clone(), valid_button.clone()]).is_ok());
        assert!(
            validate_buttons(&[valid_button.clone(), valid_button.clone(), valid_button]).is_err()
        );

        let invalid_label = ActivityButton {
            label: "".to_owned(),
            url: "https://example.com".to_owned(),
        };
        assert!(validate_buttons(&[invalid_label]).is_err());
    }

    #[test]
    fn rejects_leading_and_trailing_control_chars_before_trimming() {
        assert_eq!(
            validate_profile_name("\nGaming"),
            Err(ValidationError::InvalidControlChars("profile.name"))
        );
        assert_eq!(
            validate_profile_name("Gaming\n"),
            Err(ValidationError::InvalidControlChars("profile.name"))
        );
        assert_eq!(
            validate_profile_name("Gam\ning"),
            Err(ValidationError::InvalidControlChars("profile.name"))
        );
        assert_eq!(
            validate_profile_name("\tGaming"),
            Err(ValidationError::InvalidControlChars("profile.name"))
        );
        assert_eq!(
            validate_profile_name("Gaming\t"),
            Err(ValidationError::InvalidControlChars("profile.name"))
        );
        assert!(validate_profile_name("  Gaming  ").is_ok());

        assert_eq!(
            validate_text_field("details", "\nCoding"),
            Err(ValidationError::InvalidControlChars("details"))
        );
        assert_eq!(
            validate_text_field("details", "Coding\r"),
            Err(ValidationError::InvalidControlChars("details"))
        );
        assert_eq!(
            validate_text_field("details", "\tCoding\t"),
            Err(ValidationError::InvalidControlChars("details"))
        );
        assert!(validate_text_field("details", "  Coding  ").is_ok());

        assert_eq!(
            validate_activity_name_override("App\n"),
            Err(ValidationError::InvalidControlChars(
                "activity_name_override"
            ))
        );
        assert_eq!(
            validate_profile_id("default\n"),
            Err(ValidationError::InvalidControlChars("profile.id"))
        );
        assert_eq!(
            validate_artwork_ref(
                "large_asset",
                &ArtworkRef::PortalKey {
                    key: "my_key\n".to_owned()
                }
            ),
            Err(ValidationError::InvalidControlChars("large_asset"))
        );
        assert_eq!(
            validate_button(&ActivityButton {
                label: "Click\t".to_owned(),
                url: "https://example.com".to_owned()
            }),
            Err(ValidationError::InvalidControlChars("button.label"))
        );
        assert_eq!(
            ApplicationId::parse("123456789012345678\n"),
            Err(ApplicationIdError::InvalidFormat)
        );
    }

    #[test]
    fn validates_risk_03_exotic_ipv6_port_zero_and_dns_labels() {
        // Port 0 and non-digit port prefixes must be rejected
        assert_eq!(
            validate_https_url("url", "https://example.com:0/path", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[2606:4700:4700::1111]:0/path", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://example.com:+443/path", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );

        // Underscores in DNS hostname labels are invalid per RFC 1123
        assert_eq!(
            validate_https_url("url", "https://bad_host.example.com/path", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        // Underscores in URL paths remain allowed
        assert!(validate_https_url("url", "https://example.com/some_valid_path", 256).is_ok());

        // NAT64 local-use & NAT64 embedding private IPv4
        assert_eq!(
            validate_https_url("url", "https://[64:ff9b:1::127.0.0.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[64:ff9b::127.0.0.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[64:ff9b::10.0.0.1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );

        // 6to4 embedding private IPv4 (192.168.1.1 = c0a8:0101, 127.0.0.1 = 7f00:0001)
        assert_eq!(
            validate_https_url("url", "https://[2002:7f00:0001::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );
        assert_eq!(
            validate_https_url("url", "https://[2002:c0a8:0101::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );

        // Teredo tunneling
        assert_eq!(
            validate_https_url("url", "https://[2001::1]/", 256),
            Err(ValidationError::DisallowedUrlHost("url"))
        );

        // Legitimate public IPv6 & domain URLs remain accepted
        assert!(validate_https_url("url", "https://[2606:4700:4700::1111]:443/", 256).is_ok());
        assert!(validate_https_url("url", "https://sub-1.example.org/docs", 256).is_ok());
    }
}
