use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscordError {
    #[error("Discord desktop client is not running")]
    DiscordNotRunning,

    #[error("Discord IPC pipe is currently busy")]
    PipeBusy,

    #[error("Discord IPC handshake timed out")]
    HandshakeTimeout,

    #[error("unsupported Discord RPC protocol version: {version}")]
    ProtocolMismatch { version: u32 },

    #[error("malformed or oversized Discord IPC frame")]
    ProtocolViolation,

    #[error("Discord rejected the configured Application ID")]
    InvalidClientId,

    #[error("Discord rate-limited presence updates")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("Discord rejected the RPC request (code {code})")]
    RpcRejected { code: i32, message: String },

    #[error("not connected to Discord desktop client")]
    NotConnected,

    #[error("Discord IPC connection closed unexpectedly")]
    Disconnected,

    #[error("Discord activity clear operation timed out")]
    ClearTimeout,

    #[error("I/O error communicating with Discord: {0}")]
    Io(String),
}

impl DiscordError {
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::DiscordNotRunning
                | Self::PipeBusy
                | Self::HandshakeTimeout
                | Self::RateLimited { .. }
                | Self::Disconnected
                | Self::ClearTimeout
                | Self::Io(_)
        )
    }

    #[must_use]
    pub const fn user_message(&self) -> &'static str {
        match self {
            Self::DiscordNotRunning | Self::PipeBusy | Self::NotConnected => {
                "Discord desktop client is unavailable. DiDi will connect automatically once Discord is running."
            }
            Self::HandshakeTimeout | Self::Disconnected | Self::ClearTimeout | Self::Io(_) => {
                "Connection to Discord desktop was interrupted. Retrying automatically."
            }
            Self::ProtocolMismatch { .. } | Self::ProtocolViolation => {
                "Discord desktop returned an unexpected RPC response."
            }
            Self::InvalidClientId => {
                "Discord rejected your Application ID. Verify it in the Discord Developer Portal."
            }
            Self::RateLimited { .. } => {
                "Presence updates are temporarily paused to respect Discord rate limits."
            }
            Self::RpcRejected { .. } => {
                "Discord rejected the activity update. Review your profile fields and try again."
            }
        }
    }

    /// Returns a sanitized, static user-facing message when `CLEAR_ACTIVITY` fails or times out.
    ///
    /// Distinct from [`Self::user_message`]: never claims DiDi is retrying automatically when paused,
    /// never blames editable profile fields, and never leaks OS paths or raw peer RPC text.
    #[must_use]
    pub const fn clear_failure_message(&self) -> &'static str {
        match self {
            Self::ClearTimeout => {
                "Discord did not respond in time to confirm clearing your activity. Your previous status may remain visible until Discord refreshes."
            }
            Self::RateLimited { .. } => {
                "Discord rate-limited the activity clear request. Your previous status may remain visible briefly."
            }
            Self::Disconnected
            | Self::NotConnected
            | Self::DiscordNotRunning
            | Self::PipeBusy
            | Self::HandshakeTimeout => {
                "The connection to Discord ended before activity clearing was confirmed."
            }
            Self::RpcRejected { .. }
            | Self::ProtocolViolation
            | Self::ProtocolMismatch { .. }
            | Self::InvalidClientId => {
                "Discord rejected the activity clear request. Your previous status may still be visible in Discord."
            }
            Self::Io(_) => {
                "A local IPC error occurred before Discord confirmed clearing your activity."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DiscordError;

    #[test]
    fn retryable_classification_stops_loops_on_invalid_client_id() {
        assert!(!DiscordError::InvalidClientId.is_retryable());
        assert!(!DiscordError::ProtocolViolation.is_retryable());
        assert!(!DiscordError::ProtocolMismatch { version: 2 }.is_retryable());
        assert!(DiscordError::DiscordNotRunning.is_retryable());
        assert!(DiscordError::Disconnected.is_retryable());
    }

    #[test]
    fn clear_failure_message_is_sanitized_and_pause_accurate() {
        let variants = [
            DiscordError::DiscordNotRunning,
            DiscordError::PipeBusy,
            DiscordError::HandshakeTimeout,
            DiscordError::ProtocolMismatch { version: 2 },
            DiscordError::ProtocolViolation,
            DiscordError::InvalidClientId,
            DiscordError::RateLimited {
                retry_after_secs: Some(15),
            },
            DiscordError::RpcRejected {
                code: 5000,
                message: "RAW_SENSITIVE_RPC_MESSAGE".to_owned(),
            },
            DiscordError::NotConnected,
            DiscordError::Disconnected,
            DiscordError::ClearTimeout,
            DiscordError::Io(r"C:\Users\Alice\pipe\secret".to_owned()),
        ];

        for err in variants {
            let msg = err.clear_failure_message();
            assert!(!msg.is_empty());
            assert!(!msg.contains("Retrying automatically"));
            assert!(!msg.contains("Review your profile"));
            assert!(!msg.contains("RAW_SENSITIVE_RPC_MESSAGE"));
            assert!(!msg.contains("Alice"));
            assert!(!msg.contains(r"C:\"));
        }
    }
}
