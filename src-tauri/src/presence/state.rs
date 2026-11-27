use serde::{Deserialize, Serialize};

/// Canonical 8-state runtime lifecycle defined in Phase 1 Architecture Section 3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceState {
    Initializing,
    Disabled,
    WaitingForDiscord,
    Connecting,
    Published,
    Reconnecting,
    Error,
    Exiting,
}

impl PresenceState {
    /// Honest user-facing status label. Offline/paused/error states are never labelled "Published".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Initializing => "Starting",
            Self::Disabled => "Disabled",
            Self::WaitingForDiscord => "Waiting for Discord",
            Self::Connecting => "Connecting",
            Self::Published => "Published",
            Self::Reconnecting => "Reconnecting",
            Self::Error => "Action Required",
            Self::Exiting => "Exiting",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PresenceState;

    #[test]
    fn only_published_state_uses_published_label() {
        let non_published = [
            PresenceState::Initializing,
            PresenceState::Disabled,
            PresenceState::WaitingForDiscord,
            PresenceState::Connecting,
            PresenceState::Reconnecting,
            PresenceState::Error,
            PresenceState::Exiting,
        ];
        for state in non_published {
            assert_ne!(state.label(), "Published");
        }
        assert_eq!(PresenceState::Published.label(), "Published");
    }
}
