use std::time::Duration;

use crate::discord::DiscordError;
use crate::presence::PresenceState;

/// Base backoff schedule in seconds: [1s, 2s, 4s, 8s, 15s, 30s] (ADR D-018 / Section 3.3).
pub const BASE_BACKOFF_SCHEDULE_SECS: [u64; 6] = [1, 2, 4, 8, 15, 30];

/// Maximum backoff delay ceiling in seconds.
pub const MAX_BACKOFF_SECS: u64 = 30;

/// Bounded pseudo-random jitter factor (+/- 20%).
pub const JITTER_FACTOR: f64 = 0.20;

/// Manages retry backoff delays with bounded jitter.
#[derive(Debug, Clone)]
pub struct BackoffSchedule {
    attempts: usize,
}

impl Default for BackoffSchedule {
    fn default() -> Self {
        Self::new()
    }
}

impl BackoffSchedule {
    #[must_use]
    pub const fn new() -> Self {
        Self { attempts: 0 }
    }

    #[must_use]
    pub const fn attempts(&self) -> usize {
        self.attempts
    }

    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    /// Computes the base delay before jitter for the current attempt count.
    #[must_use]
    pub fn current_base_delay(&self) -> Duration {
        let index = self.attempts.min(BASE_BACKOFF_SCHEDULE_SECS.len() - 1);
        Duration::from_secs(BASE_BACKOFF_SCHEDULE_SECS[index])
    }

    /// Returns the next delay with pseudo-random jitter applied, advancing the attempt counter.
    pub fn next_delay(&mut self) -> Duration {
        let base = self.current_base_delay();
        self.attempts = self.attempts.saturating_add(1);
        apply_jitter(base, get_pseudo_jitter_ratio(self.attempts))
    }

    /// Deterministic delay calculation for unit testing without pseudo-randomness.
    #[must_use]
    pub fn delay_with_fixed_ratio(&self, jitter_ratio: f64) -> Duration {
        apply_jitter(self.current_base_delay(), jitter_ratio)
    }
}

/// Applies jitter ratio in range `[-JITTER_FACTOR, +JITTER_FACTOR]` to the base duration.
#[must_use]
pub fn apply_jitter(base: Duration, ratio: f64) -> Duration {
    let clamped_ratio = ratio.clamp(-JITTER_FACTOR, JITTER_FACTOR);
    let base_ms = base.as_millis() as f64;
    let jittered_ms = (base_ms * (1.0 + clamped_ratio)).max(100.0);
    Duration::from_millis(jittered_ms as u64)
}

/// Deterministic pseudo-random jitter ratio generator using linear congruential hashing
/// of the attempt count and current timestamp.
#[must_use]
fn get_pseudo_jitter_ratio(attempt: usize) -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(42);
    // Mix attempt and nanos into a normalized [-0.20, +0.20] range
    let hash = (nanos ^ (attempt as u32).wrapping_mul(0x9E37_79B9)) % 1000;
    // Normalized 0.0 .. 1.0 -> map to -JITTER_FACTOR .. +JITTER_FACTOR
    let normalized = (hash as f64) / 1000.0;
    (normalized * 2.0 - 1.0) * JITTER_FACTOR
}

/// Reconnect state machine enforcing Section 3.1 & 3.3 lifecycle contracts.
#[derive(Debug, Clone)]
pub struct PresenceReconnectStateMachine {
    state: PresenceState,
    backoff: BackoffSchedule,
    consecutive_failures: usize,
    ever_published: bool,
}

impl Default for PresenceReconnectStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl PresenceReconnectStateMachine {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: PresenceState::Initializing,
            backoff: BackoffSchedule::new(),
            consecutive_failures: 0,
            ever_published: false,
        }
    }

    #[must_use]
    pub const fn state(&self) -> PresenceState {
        self.state
    }

    #[must_use]
    pub const fn consecutive_failures(&self) -> usize {
        self.consecutive_failures
    }

    /// Invoked when initialization completes and presence is enabled.
    pub fn on_initialized(&mut self, enabled: bool) -> PresenceState {
        self.state = if enabled {
            PresenceState::WaitingForDiscord
        } else {
            PresenceState::Disabled
        };
        self.state
    }

    /// Invoked when a pipe slot is detected and connection/handshake begins.
    pub fn on_pipe_detected(&mut self) -> PresenceState {
        self.state = PresenceState::Connecting;
        self.state
    }

    /// Invoked when handshake and SET_ACTIVITY succeed, publishing presence.
    pub fn on_publish_success(&mut self) -> PresenceState {
        self.state = PresenceState::Published;
        self.ever_published = true;
        self.backoff.reset();
        self.consecutive_failures = 0;
        self.state
    }

    /// Invoked when an unexpected pipe EOF, broken pipe, or RPC failure occurs.
    /// Returns the new state and the backoff duration (zero if non-retryable).
    pub fn on_disconnect(&mut self, error: &DiscordError) -> (PresenceState, Duration) {
        if !error.is_retryable() {
            self.state = PresenceState::Error;
            return (self.state, Duration::ZERO);
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = self.backoff.next_delay();

        // First 2 failures after Published show Reconnecting; subsequent failures or pre-publish show WaitingForDiscord
        self.state = if self.ever_published && self.consecutive_failures <= 2 {
            PresenceState::Reconnecting
        } else {
            PresenceState::WaitingForDiscord
        };

        (self.state, delay)
    }

    /// Invoked when the user manually pauses presence.
    pub fn on_user_pause(&mut self) -> PresenceState {
        self.state = PresenceState::Disabled;
        self.ever_published = false;
        self.backoff.reset();
        self.state
    }

    /// Invoked when the user resumes presence or fixes configuration.
    pub fn on_user_resume(&mut self) -> PresenceState {
        self.state = PresenceState::WaitingForDiscord;
        self.ever_published = false;
        self.backoff.reset();
        self.consecutive_failures = 0;
        self.state
    }

    /// Invoked when the user clicks "Retry Now" on an Error or Waiting banner.
    pub fn on_retry_now(&mut self) -> PresenceState {
        self.state = PresenceState::Connecting;
        self.ever_published = false;
        self.backoff.reset();
        self.consecutive_failures = 0;
        self.state
    }

    /// Invoked when the application begins clean shutdown.
    pub fn on_exit(&mut self) -> PresenceState {
        self.state = PresenceState::Exiting;
        self.state
    }

    /// Invoked when startup configuration integrity fails (e.g. unsupported future schema).
    pub fn on_startup_error(&mut self) -> PresenceState {
        self.state = PresenceState::Error;
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule_follows_exponential_schedule_and_caps_at_30s() {
        let mut schedule = BackoffSchedule::new();
        assert_eq!(schedule.current_base_delay(), Duration::from_secs(1));

        let expected_bases = [1, 2, 4, 8, 15, 30, 30, 30];
        for expected in expected_bases {
            assert_eq!(schedule.current_base_delay(), Duration::from_secs(expected));
            let _ = schedule.next_delay();
        }
    }

    #[test]
    fn jitter_bounds_are_strictly_within_plus_minus_20_percent() {
        let base = Duration::from_secs(10);
        let min_allowed = Duration::from_secs(8);
        let max_allowed = Duration::from_secs(12);

        for ratio_percent in -30..=30 {
            let ratio = (ratio_percent as f64) / 100.0;
            let jittered = apply_jitter(base, ratio);
            assert!(
                jittered >= min_allowed && jittered <= max_allowed,
                "Jittered duration {jittered:?} outside expected bounds [8s, 12s] for ratio {ratio}"
            );
        }
    }

    #[test]
    fn state_machine_lifecycle_transitions() {
        let mut sm = PresenceReconnectStateMachine::new();
        assert_eq!(sm.state(), PresenceState::Initializing);

        // Transition to WaitingForDiscord on startup
        assert_eq!(sm.on_initialized(true), PresenceState::WaitingForDiscord);

        // Pipe detected -> Connecting
        assert_eq!(sm.on_pipe_detected(), PresenceState::Connecting);

        // Published
        assert_eq!(sm.on_publish_success(), PresenceState::Published);
        assert_eq!(sm.consecutive_failures(), 0);

        // First disconnect -> Reconnecting
        let (s1, d1) = sm.on_disconnect(&DiscordError::Disconnected);
        assert_eq!(s1, PresenceState::Reconnecting);
        assert!(d1 >= Duration::from_millis(800) && d1 <= Duration::from_millis(1200));
        assert_eq!(sm.consecutive_failures(), 1);

        // Second disconnect -> Reconnecting
        let (s2, _) = sm.on_disconnect(&DiscordError::Disconnected);
        assert_eq!(s2, PresenceState::Reconnecting);
        assert_eq!(sm.consecutive_failures(), 2);

        // Third disconnect -> transitions to WaitingForDiscord
        let (s3, _) = sm.on_disconnect(&DiscordError::Disconnected);
        assert_eq!(s3, PresenceState::WaitingForDiscord);
        assert_eq!(sm.consecutive_failures(), 3);

        // User pause -> Disabled
        assert_eq!(sm.on_user_pause(), PresenceState::Disabled);

        // User resume -> WaitingForDiscord
        assert_eq!(sm.on_user_resume(), PresenceState::WaitingForDiscord);
        assert_eq!(sm.consecutive_failures(), 0);
    }

    #[test]
    fn initial_failure_without_publish_stays_waiting_for_discord() {
        let mut sm = PresenceReconnectStateMachine::new();
        assert_eq!(sm.on_initialized(true), PresenceState::WaitingForDiscord);
        assert_eq!(sm.on_pipe_detected(), PresenceState::Connecting);

        // Disconnecting before publish ever succeeded must NOT transition to Reconnecting
        let (state, delay) = sm.on_disconnect(&DiscordError::DiscordNotRunning);
        assert_eq!(state, PresenceState::WaitingForDiscord);
        assert!(delay >= Duration::from_millis(800) && delay <= Duration::from_millis(1200));
        assert_eq!(sm.consecutive_failures(), 1);
    }

    #[test]
    fn non_retryable_error_stops_retry_loop_immediately() {
        let mut sm = PresenceReconnectStateMachine::new();
        sm.on_initialized(true);
        sm.on_pipe_detected();

        // Invalid client ID (4000) must transition immediately to Error with ZERO delay
        let (state, delay) = sm.on_disconnect(&DiscordError::InvalidClientId);
        assert_eq!(state, PresenceState::Error);
        assert_eq!(delay, Duration::ZERO);

        // Protocol violation also transitions to Error with ZERO delay
        let (state, delay) = sm.on_disconnect(&DiscordError::ProtocolViolation);
        assert_eq!(state, PresenceState::Error);
        assert_eq!(delay, Duration::ZERO);
    }
}
