use serde::{Deserialize, Serialize};

use crate::discord::DiscordError;
use crate::presence::{ApplicationId, ArtworkRef, PresenceActivity};

/// Payload sent in Opcode 0 (HANDSHAKE) to initiate an RPC connection.
#[derive(Debug, Clone, Serialize)]
pub struct HandshakePayload<'a> {
    pub v: u32,
    pub client_id: &'a str,
}

impl<'a> HandshakePayload<'a> {
    #[must_use]
    pub fn new(app_id: &'a ApplicationId) -> Self {
        Self {
            v: 1,
            client_id: app_id.as_str(),
        }
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, DiscordError> {
        serde_json::to_vec(self).map_err(|e| DiscordError::Io(e.to_string()))
    }
}

/// Zero-PII READY response deserializer (ADR D-022).
///
/// Deliberately omits Discord user object, avatar, discriminator, and username
/// from parsing to guarantee zero PII is ever retained in memory or disk.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadyPayload {
    pub cmd: String,
    pub evt: Option<String>,
    pub data: Option<ReadyData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadyData {
    pub v: u32,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Generic response frame structure returned by Discord RPC.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcResponse {
    pub cmd: Option<String>,
    pub evt: Option<String>,
    pub nonce: Option<String>,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcErrorData {
    pub code: i32,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub retry_after: Option<f64>,
}

impl RpcErrorData {
    #[must_use]
    pub fn into_discord_error(self) -> DiscordError {
        match self.code {
            4000 => DiscordError::InvalidClientId,
            4002 => DiscordError::RateLimited {
                retry_after_secs: self
                    .retry_after
                    .filter(|secs| secs.is_finite() && *secs > 0.0)
                    .map(|secs| secs.ceil() as u64),
            },
            4004 => DiscordError::ProtocolMismatch { version: 0 },
            code => DiscordError::RpcRejected {
                code,
                message: self.message,
            },
        }
    }
}

impl RpcResponse {
    /// Validates whether the response is a successful ACK for the given expected nonce.
    pub fn check_ack(&self, expected_nonce: &str) -> Result<(), DiscordError> {
        if let Some(evt) = &self.evt {
            if evt == "ERROR" {
                if let Some(data) = &self.data {
                    if let Ok(err) = serde_json::from_value::<RpcErrorData>(data.clone()) {
                        return Err(err.into_discord_error());
                    }
                }
                return Err(DiscordError::RpcRejected {
                    code: -1,
                    message: "Discord returned an unspecified error".to_owned(),
                });
            }
        }

        if let Some(nonce) = &self.nonce {
            if nonce != expected_nonce {
                return Err(DiscordError::ProtocolViolation);
            }
        }

        Ok(())
    }
}

/// Wire layout for Unix start/end timestamps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireTimestamps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
}

/// Wire layout for Rich Presence artwork assets and tooltips.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WireAssets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub large_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_url: Option<String>,
}

/// Wire layout for clickable action buttons.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireButton {
    pub label: String,
    pub url: String,
}

/// Complete wire activity structure sent to Discord in `SET_ACTIVITY`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireActivity {
    #[serde(rename = "type")]
    pub activity_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_display_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamps: Option<WireTimestamps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<WireAssets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buttons: Option<Vec<WireButton>>,
}

impl WireActivity {
    #[must_use]
    pub fn from_domain(activity: &PresenceActivity) -> Self {
        let timestamps = if activity.start_timestamp.is_some() || activity.end_timestamp.is_some() {
            Some(WireTimestamps {
                start: activity.start_timestamp,
                end: activity.end_timestamp,
            })
        } else {
            None
        };

        let large_image = match &activity.large_asset {
            ArtworkRef::None => None,
            ArtworkRef::PortalKey { key } => Some(key.clone()),
            ArtworkRef::ExternalUrl { url } => Some(url.clone()),
        };
        let small_image = match &activity.small_asset {
            ArtworkRef::None => None,
            ArtworkRef::PortalKey { key } => Some(key.clone()),
            ArtworkRef::ExternalUrl { url } => Some(url.clone()),
        };

        let assets = if large_image.is_some()
            || small_image.is_some()
            || activity.large_text.is_some()
            || activity.small_text.is_some()
            || activity.large_url.is_some()
            || activity.small_url.is_some()
        {
            Some(WireAssets {
                large_image,
                large_text: activity.large_text.clone(),
                large_url: activity.large_url.clone(),
                small_image,
                small_text: activity.small_text.clone(),
                small_url: activity.small_url.clone(),
            })
        } else {
            None
        };

        let buttons = if activity.buttons.is_empty() {
            None
        } else {
            Some(
                activity
                    .buttons
                    .iter()
                    .take(2)
                    .map(|b| WireButton {
                        label: b.label.clone(),
                        url: b.url.clone(),
                    })
                    .collect(),
            )
        };

        Self {
            activity_type: activity.activity_type.wire_code(),
            name: activity.name_override.clone(),
            details: activity.details.clone(),
            details_url: activity.details_url.clone(),
            state: activity.state.clone(),
            state_url: activity.state_url.clone(),
            status_display_type: activity.status_display_type.map(|s| s.wire_code()),
            timestamps,
            assets,
            buttons,
        }
    }
}

/// Command payload for `SET_ACTIVITY`. When `activity` is `None`, serializes to `"activity": null` (clearing activity).
#[derive(Debug, Clone, Serialize)]
pub struct SetActivityCommand<'a> {
    pub cmd: &'static str,
    pub args: SetActivityArgs,
    pub nonce: &'a str,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetActivityArgs {
    pub pid: u32,
    pub activity: Option<WireActivity>,
}

impl<'a> SetActivityCommand<'a> {
    #[must_use]
    pub fn set(pid: u32, activity: WireActivity, nonce: &'a str) -> Self {
        Self {
            cmd: "SET_ACTIVITY",
            args: SetActivityArgs {
                pid,
                activity: Some(activity),
            },
            nonce,
        }
    }

    #[must_use]
    pub fn clear(pid: u32, nonce: &'a str) -> Self {
        Self {
            cmd: "SET_ACTIVITY",
            args: SetActivityArgs {
                pid,
                activity: None,
            },
            nonce,
        }
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, DiscordError> {
        serde_json::to_vec(self).map_err(|e| DiscordError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{ActivityButton, ActivityType, StatusDisplayType, TimestampMode};

    #[test]
    fn handshake_serializes_cleanly() -> Result<(), DiscordError> {
        let app_id = ApplicationId::parse("123456789012345678")
            .map_err(|_| DiscordError::InvalidClientId)?;
        let payload = HandshakePayload::new(&app_id);
        let bytes = payload.to_json_bytes()?;
        let json_str = String::from_utf8_lossy(&bytes);

        assert!(json_str.contains("\"v\":1"));
        assert!(json_str.contains("\"client_id\":\"123456789012345678\""));
        Ok(())
    }

    #[test]
    fn ready_payload_deserializes_zero_pii() -> Result<(), serde_json::Error> {
        let raw = r#"{
            "cmd": "DISPATCH",
            "data": {
                "v": 1,
                "config": {"cdn_host": "cdn.discordapp.com"},
                "user": {
                    "id": "999999999999999999",
                    "username": "secret_user",
                    "avatar": "deadbeef"
                }
            },
            "evt": "READY"
        }"#;

        let ready: ReadyPayload = serde_json::from_str(raw)?;
        assert_eq!(ready.cmd, "DISPATCH");
        assert_eq!(ready.evt.as_deref(), Some("READY"));
        assert_eq!(ready.data.as_ref().map(|d| d.v), Some(1));
        Ok(())
    }

    #[test]
    fn set_activity_serializes_with_all_fields() -> Result<(), DiscordError> {
        let activity = PresenceActivity {
            activity_type: ActivityType::Listening,
            status_display_type: Some(StatusDisplayType::Details),
            name_override: Some("Custom Synth".to_owned()),
            details: Some("Coding DiDi".to_owned()),
            details_url: Some("https://example.com/details".to_owned()),
            state: Some("Focus Mode".to_owned()),
            state_url: Some("https://example.com/state".to_owned()),
            large_asset: ArtworkRef::PortalKey {
                key: "large_art".to_owned(),
            },
            large_text: Some("Album Art".to_owned()),
            large_url: Some("https://example.com/large".to_owned()),
            small_asset: ArtworkRef::ExternalUrl {
                url: "https://example.com/small.png".to_owned(),
            },
            small_text: Some("Play State".to_owned()),
            small_url: None,
            timestamp_mode: TimestampMode::None,
            start_timestamp: Some(1700000000),
            end_timestamp: None,
            buttons: vec![ActivityButton {
                label: "Visit Project".to_owned(),
                url: "https://example.com".to_owned(),
            }],
        };

        let wire = WireActivity::from_domain(&activity);
        let cmd = SetActivityCommand::set(1234, wire, "test-nonce-1234");
        let bytes = cmd.to_json_bytes()?;
        let json_str = String::from_utf8_lossy(&bytes);

        assert!(json_str.contains("\"cmd\":\"SET_ACTIVITY\""));
        assert!(json_str.contains("\"pid\":1234"));
        assert!(json_str.contains("\"type\":2"));
        assert!(json_str.contains("\"name\":\"Custom Synth\""));
        assert!(json_str.contains("\"details\":\"Coding DiDi\""));
        assert!(json_str.contains("\"nonce\":\"test-nonce-1234\""));
        assert!(json_str.contains("\"large_image\":\"large_art\""));
        assert!(json_str.contains("\"small_image\":\"https://example.com/small.png\""));
        assert!(json_str.contains("\"start\":1700000000"));
        assert!(json_str.contains("\"label\":\"Visit Project\""));
        Ok(())
    }

    #[test]
    fn clear_activity_serializes_with_null_activity() -> Result<(), DiscordError> {
        let cmd = SetActivityCommand::clear(5678, "clear-nonce-999");
        let bytes = cmd.to_json_bytes()?;
        let json_str = String::from_utf8_lossy(&bytes);

        assert!(json_str.contains("\"cmd\":\"SET_ACTIVITY\""));
        assert!(json_str.contains("\"pid\":5678"));
        assert!(json_str.contains("\"activity\":null"));
        assert!(json_str.contains("\"nonce\":\"clear-nonce-999\""));
        Ok(())
    }

    #[test]
    fn rpc_response_validates_nonce_and_error_codes() {
        let ok_resp = RpcResponse {
            cmd: Some("SET_ACTIVITY".to_owned()),
            evt: None,
            nonce: Some("nonce-abc".to_owned()),
            data: Some(serde_json::json!({})),
        };
        assert_eq!(ok_resp.check_ack("nonce-abc"), Ok(()));
        assert_eq!(
            ok_resp.check_ack("nonce-wrong"),
            Err(DiscordError::ProtocolViolation)
        );

        let err_resp = RpcResponse {
            cmd: Some("SET_ACTIVITY".to_owned()),
            evt: Some("ERROR".to_owned()),
            nonce: Some("nonce-abc".to_owned()),
            data: Some(serde_json::json!({
                "code": 4000,
                "message": "Invalid client_id"
            })),
        };
        assert_eq!(
            err_resp.check_ack("nonce-abc"),
            Err(DiscordError::InvalidClientId)
        );

        let ratelimit_resp = RpcResponse {
            cmd: Some("SET_ACTIVITY".to_owned()),
            evt: Some("ERROR".to_owned()),
            nonce: Some("nonce-rl".to_owned()),
            data: Some(serde_json::json!({
                "code": 4002,
                "retry_after": 4.2
            })),
        };
        assert_eq!(
            ratelimit_resp.check_ack("nonce-rl"),
            Err(DiscordError::RateLimited {
                retry_after_secs: Some(5)
            })
        );

        let version_resp = RpcResponse {
            cmd: Some("SET_ACTIVITY".to_owned()),
            evt: Some("ERROR".to_owned()),
            nonce: Some("nonce-ver".to_owned()),
            data: Some(serde_json::json!({
                "code": 4004,
                "message": "Invalid Version"
            })),
        };
        assert_eq!(
            version_resp.check_ack("nonce-ver"),
            Err(DiscordError::ProtocolMismatch { version: 0 })
        );
    }
}
