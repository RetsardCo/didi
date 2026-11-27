use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Standard debounce window for presence edits (ADR D-018 / Section 2.4).
pub const DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

/// Rolling window duration enforced for client-side rate limiting (20 seconds).
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(20);

/// Maximum SET_ACTIVITY updates allowed within a rolling 20-second window.
///
/// Discord documents a maximum of 5 presence updates per 20 seconds (`gateway-events.mdx`).
/// DiDi enforces a maximum of 4 updates to guarantee 1 slot headroom for immediate
/// Pause/Clear or explicit Exit commands (ADR D-018 / Section 2.4).
pub const MAX_UPDATES_PER_WINDOW: usize = 4;

/// Pure sliding-window rate limiter tracking timestamps of recent updates.
#[derive(Debug, Clone)]
pub struct RollingRateLimiter {
    max_updates: usize,
    window: Duration,
    timestamps: VecDeque<Instant>,
}

impl Default for RollingRateLimiter {
    fn default() -> Self {
        Self::new(MAX_UPDATES_PER_WINDOW, RATE_LIMIT_WINDOW)
    }
}

impl RollingRateLimiter {
    #[must_use]
    pub fn new(max_updates: usize, window: Duration) -> Self {
        Self {
            max_updates,
            window,
            timestamps: VecDeque::with_capacity(max_updates.saturating_add(1)),
        }
    }

    /// Removes expired timestamps that fall outside `[now - window, now]`.
    fn prune(&mut self, now: Instant) {
        while let Some(&oldest) = self.timestamps.front() {
            if now.saturating_duration_since(oldest) >= self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Checks if a new update is allowed at time `now`.
    #[must_use]
    pub fn can_proceed(&self, now: Instant) -> bool {
        let active = self.active_count(now);
        active < self.max_updates
    }

    /// Returns the number of active updates within the rolling window at time `now`.
    #[must_use]
    pub fn active_count(&self, now: Instant) -> usize {
        self.timestamps
            .iter()
            .filter(|&&t| now.saturating_duration_since(t) < self.window)
            .count()
    }

    /// Calculates the duration to wait until a rate-limit slot opens up, or `None` if available now.
    #[must_use]
    pub fn time_until_available(&self, now: Instant) -> Option<Duration> {
        let active = self.active_count(now);
        if active < self.max_updates {
            return None;
        }

        // The oldest active timestamp in the chronological queue
        let oldest_active = self.timestamps.front()?;
        let elapsed = now.saturating_duration_since(*oldest_active);
        if elapsed < self.window {
            Some(self.window - elapsed)
        } else {
            None
        }
    }

    /// Records an update at time `now`. Returns `Ok(())` if recorded, or `Err(Duration)`
    /// containing the remaining cooldown time if rate limited.
    pub fn record(&mut self, now: Instant) -> Result<(), Duration> {
        self.prune(now);
        if self.timestamps.len() < self.max_updates {
            self.timestamps.push_back(now);
            Ok(())
        } else if let Some(wait_time) = self.time_until_available(now) {
            Err(wait_time)
        } else {
            // Edge case: pruned timestamp frees slot exactly at boundary
            self.timestamps.push_back(now);
            Ok(())
        }
    }

    /// Resets all recorded timestamps.
    pub fn reset(&mut self) {
        self.timestamps.clear();
    }
}

/// Coalescing buffer storing at most one pending item where the latest update always wins.
#[derive(Debug, Clone, Default)]
pub struct CoalescingBuffer<T> {
    pending: Option<T>,
}

impl<T> CoalescingBuffer<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self { pending: None }
    }

    /// Stores a new item, replacing any existing pending item ("latest wins").
    pub fn set(&mut self, item: T) {
        self.pending = Some(item);
    }

    /// Takes the pending item out of the buffer, leaving `None`.
    pub fn take(&mut self) -> Option<T> {
        self.pending.take()
    }

    /// Inspects the pending item without consuming it.
    #[must_use]
    pub const fn peek(&self) -> Option<&T> {
        self.pending.as_ref()
    }

    /// Clears any pending item.
    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// Returns `true` if a pending item is buffered.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_limiter_permits_up_to_max_updates_and_rejects_excess() {
        let mut limiter = RollingRateLimiter::new(4, Duration::from_secs(20));
        let base = Instant::now();

        // 4 updates within 3 seconds should all succeed
        assert_eq!(limiter.record(base), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(1)), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(2)), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(3)), Ok(()));
        assert_eq!(limiter.active_count(base + Duration::from_secs(3)), 4);
        assert!(!limiter.can_proceed(base + Duration::from_secs(3)));

        // 5th update at t=4s should be rejected with remaining cooldown
        let fifth_res = limiter.record(base + Duration::from_secs(4));
        assert_eq!(fifth_res, Err(Duration::from_secs(16)));
        // Oldest is at t=0s, so it frees at t=20s. At t=4s, wait is 16s.
        assert_eq!(
            limiter.time_until_available(base + Duration::from_secs(4)),
            Some(Duration::from_secs(16))
        );
    }

    #[test]
    fn rolling_limiter_frees_slot_after_window_expires() {
        let mut limiter = RollingRateLimiter::new(4, Duration::from_secs(20));
        let base = Instant::now();

        assert_eq!(limiter.record(base), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(1)), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(2)), Ok(()));
        assert_eq!(limiter.record(base + Duration::from_secs(3)), Ok(()));

        // At t=20.1s, the first timestamp (t=0) has expired
        let t_after = base + Duration::from_millis(20_100);
        assert!(limiter.can_proceed(t_after));
        assert_eq!(limiter.time_until_available(t_after), None);
        assert_eq!(limiter.record(t_after), Ok(()));
        assert_eq!(limiter.active_count(t_after), 4);
    }

    #[test]
    fn rolling_limiter_exact_window_boundary() {
        let mut limiter = RollingRateLimiter::new(4, Duration::from_secs(20));
        let base = Instant::now();

        for i in 0..4 {
            assert_eq!(limiter.record(base + Duration::from_secs(i)), Ok(()));
        }

        // At t=19.999s, the first timestamp (t=0s) has NOT expired (remaining 1ms cooldown)
        let t_before = base + Duration::from_millis(19_999);
        assert!(!limiter.can_proceed(t_before));
        assert_eq!(
            limiter.time_until_available(t_before),
            Some(Duration::from_millis(1))
        );
        assert_eq!(limiter.record(t_before), Err(Duration::from_millis(1)));

        // At exact t=20.000s, the first timestamp (t=0s) expires (elapsed == 20s)
        let t_exact = base + Duration::from_secs(20);
        assert!(limiter.can_proceed(t_exact));
        assert_eq!(limiter.time_until_available(t_exact), None);
        assert_eq!(limiter.record(t_exact), Ok(()));
    }

    #[test]
    fn rolling_limiter_reset_clears_history() {
        let mut limiter = RollingRateLimiter::default();
        let base = Instant::now();

        for i in 0..4 {
            assert_eq!(
                limiter.record(base + Duration::from_millis(i * 100)),
                Ok(())
            );
        }
        assert!(!limiter.can_proceed(base + Duration::from_millis(400)));

        limiter.reset();
        assert!(limiter.can_proceed(base + Duration::from_millis(400)));
        assert_eq!(limiter.active_count(base + Duration::from_millis(400)), 0);
    }

    #[test]
    fn coalescing_buffer_latest_wins_semantics() {
        let mut buffer = CoalescingBuffer::<&'static str>::new();
        assert!(!buffer.has_pending());
        assert_eq!(buffer.peek(), None);

        buffer.set("update_1");
        assert!(buffer.has_pending());
        assert_eq!(buffer.peek(), Some(&"update_1"));

        // Rapid subsequent edits replace previous snapshot
        buffer.set("update_2");
        buffer.set("update_3");
        assert_eq!(buffer.peek(), Some(&"update_3"));

        // Consumption takes the latest and leaves buffer empty
        assert_eq!(buffer.take(), Some("update_3"));
        assert!(!buffer.has_pending());
        assert_eq!(buffer.take(), None);
    }

    #[test]
    fn coalescing_buffer_clear_removes_pending() {
        let mut buffer = CoalescingBuffer::<i32>::new();
        buffer.set(42);
        assert!(buffer.has_pending());
        buffer.clear();
        assert!(!buffer.has_pending());
        assert_eq!(buffer.take(), None);
    }
}
