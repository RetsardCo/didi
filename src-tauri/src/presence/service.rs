use std::pin::Pin;
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Sleep;

use crate::discord::{
    AdapterEvent, DiscordError, DiscordPresenceAdapter, PresenceReconnectStateMachine, SessionInfo,
};
use crate::errors::DiDiError;
use crate::presence::{
    rate_limit::{CoalescingBuffer, RollingRateLimiter, DEBOUNCE_DURATION},
    ApplicationId, PresenceActivity, PresenceState, TimestampMode,
};

const DEFAULT_RATE_LIMIT_RETRY_SECS: u64 = 25;

/// Commands accepted by the background Presence Service Coordinator actor.
#[derive(Debug)]
pub enum ServiceCommand {
    /// Request to set or update activity (subject to 500 ms debounce & 4/20s rate limit).
    SetActivity(Box<PresenceActivity>),
    /// Immediate request to clear active activity (cancels pending debounced set).
    ClearActivity,
    /// Pause presence (clears activity, disconnects pipe, cancels timers).
    Pause,
    /// Resume presence (resets backoff, begins connecting if App ID is configured).
    Resume,
    /// Atomically resume presence with the active profile and resolved Application ID.
    ResumeWithProfile {
        app_id: Option<ApplicationId>,
        activity: Option<Box<PresenceActivity>>,
    },
    /// Set or update the Discord Application ID.
    SetApplicationId(Option<ApplicationId>),
    /// Atomically apply an active profile's Application ID and activity together.
    ApplyProfile {
        app_id: Option<ApplicationId>,
        activity: Box<PresenceActivity>,
    },
    /// User requested immediate retry from an Error or Waiting banner.
    RetryNow,
    /// Clean shutdown of the service actor, acknowledging completion after teardown.
    Shutdown { completion: oneshot::Sender<()> },
}

/// Observable status snapshot broadcast by the Presence Service Coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresenceServiceStatus {
    pub state: PresenceState,
    pub status_label: &'static str,
    pub masked_app_id: Option<String>,
    pub is_connected: bool,
    pub active_activity: Option<PresenceActivity>,
    pub pending_activity: Option<PresenceActivity>,
    pub pipe_slot: Option<u8>,
    pub protocol_version: Option<u32>,
    pub consecutive_failures: usize,
    pub last_error: Option<String>,
}

/// Thread-safe handle to coordinate and observe the Presence Service.
#[derive(Clone)]
pub struct PresenceServiceHandle {
    cmd_tx: mpsc::Sender<ServiceCommand>,
    status_rx: watch::Receiver<PresenceServiceStatus>,
}

impl PresenceServiceHandle {
    #[must_use]
    pub fn status(&self) -> PresenceServiceStatus {
        self.status_rx.borrow().clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<PresenceServiceStatus> {
        self.status_rx.clone()
    }

    pub async fn send_command(&self, cmd: ServiceCommand) -> Result<(), DiDiError> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|err| DiDiError::Lifecycle(err.to_string()))
    }

    pub async fn set_activity(&self, activity: PresenceActivity) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::SetActivity(Box::new(activity)))
            .await
    }

    pub async fn clear_activity(&self) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::ClearActivity).await
    }

    pub async fn pause(&self) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::Pause).await
    }

    pub async fn resume(&self) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::Resume).await
    }

    pub async fn resume_with_profile(
        &self,
        app_id: Option<ApplicationId>,
        activity: Option<PresenceActivity>,
    ) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::ResumeWithProfile {
            app_id,
            activity: activity.map(Box::new),
        })
        .await
    }

    pub async fn retry_now(&self) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::RetryNow).await
    }

    pub async fn set_application_id(&self, app_id: Option<ApplicationId>) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::SetApplicationId(app_id))
            .await
    }

    pub async fn apply_profile(
        &self,
        app_id: Option<ApplicationId>,
        activity: PresenceActivity,
    ) -> Result<(), DiDiError> {
        self.send_command(ServiceCommand::ApplyProfile {
            app_id,
            activity: Box::new(activity),
        })
        .await
    }

    pub async fn shutdown(&self) -> Result<(), DiDiError> {
        let (tx, rx) = oneshot::channel();
        self.send_command(ServiceCommand::Shutdown { completion: tx })
            .await?;
        let _ = rx.await;
        Ok(())
    }
}

/// Background actor coordinating presence state, debounce, rate limiting, and adapter I/O.
pub struct PresenceService {
    adapter: Box<dyn DiscordPresenceAdapter>,
    state_machine: PresenceReconnectStateMachine,
    rate_limiter: RollingRateLimiter,
    buffer: CoalescingBuffer<PresenceActivity>,
    application_id: Option<ApplicationId>,
    enabled: bool,
    connected: bool,
    active_activity: Option<PresenceActivity>,
    session_info: Option<SessionInfo>,
    session_start_unix: Option<u64>,
    rate_limit_until: Option<Instant>,
    last_error: Option<String>,
    cmd_rx: mpsc::Receiver<ServiceCommand>,
    status_tx: watch::Sender<PresenceServiceStatus>,
    #[cfg(test)]
    clock_override_unix: Option<Arc<AtomicU64>>,
}

impl PresenceService {
    #[must_use]
    pub fn new(
        adapter: Box<dyn DiscordPresenceAdapter>,
        application_id: Option<ApplicationId>,
        enabled: bool,
    ) -> (Self, PresenceServiceHandle) {
        Self::with_initial_activity(adapter, application_id, enabled, None)
    }

    #[must_use]
    pub fn with_initial_activity(
        adapter: Box<dyn DiscordPresenceAdapter>,
        application_id: Option<ApplicationId>,
        enabled: bool,
        initial_activity: Option<PresenceActivity>,
    ) -> (Self, PresenceServiceHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let mut state_machine = PresenceReconnectStateMachine::new();
        let initial_state = state_machine.on_initialized(enabled && application_id.is_some());

        let mut buffer = CoalescingBuffer::new();
        if enabled {
            if let Some(activity) = initial_activity {
                buffer.set(activity);
            }
        }

        let initial_status = PresenceServiceStatus {
            state: initial_state,
            status_label: initial_state.label(),
            masked_app_id: application_id.as_ref().map(ApplicationId::masked),
            is_connected: false,
            active_activity: None,
            pending_activity: buffer.peek().cloned(),
            pipe_slot: None,
            protocol_version: None,
            consecutive_failures: 0,
            last_error: None,
        };

        let (status_tx, status_rx) = watch::channel(initial_status);
        let handle = PresenceServiceHandle { cmd_tx, status_rx };

        let service = Self {
            adapter,
            state_machine,
            rate_limiter: RollingRateLimiter::default(),
            buffer,
            application_id,
            enabled,
            connected: false,
            active_activity: None,
            session_info: None,
            session_start_unix: None,
            rate_limit_until: None,
            last_error: None,
            cmd_rx,
            status_tx,
            #[cfg(test)]
            clock_override_unix: None,
        };

        (service, handle)
    }

    #[must_use]
    pub fn with_startup_error(
        adapter: Box<dyn DiscordPresenceAdapter>,
        error_message: String,
    ) -> (Self, PresenceServiceHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let mut state_machine = PresenceReconnectStateMachine::new();
        let initial_state = state_machine.on_startup_error();

        let initial_status = PresenceServiceStatus {
            state: initial_state,
            status_label: initial_state.label(),
            masked_app_id: None,
            is_connected: false,
            active_activity: None,
            pending_activity: None,
            pipe_slot: None,
            protocol_version: None,
            consecutive_failures: 0,
            last_error: Some(error_message.clone()),
        };

        let (status_tx, status_rx) = watch::channel(initial_status);
        let handle = PresenceServiceHandle { cmd_tx, status_rx };

        let service = Self {
            adapter,
            state_machine,
            rate_limiter: RollingRateLimiter::default(),
            buffer: CoalescingBuffer::new(),
            application_id: None,
            enabled: false,
            connected: false,
            active_activity: None,
            session_info: None,
            session_start_unix: None,
            rate_limit_until: None,
            last_error: Some(error_message),
            cmd_rx,
            status_tx,
            #[cfg(test)]
            clock_override_unix: None,
        };

        (service, handle)
    }

    fn current_unix_secs(&self) -> u64 {
        #[cfg(test)]
        if let Some(ref clock) = self.clock_override_unix {
            return clock.load(Ordering::SeqCst);
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn broadcast_status(&self) {
        broadcast_status_snapshot(
            &self.status_tx,
            &self.state_machine,
            self.application_id.as_ref(),
            self.connected,
            &self.active_activity,
            &self.buffer,
            self.session_info.as_ref(),
            self.last_error.as_deref(),
        );
    }

    /// Primary service actor event loop.
    pub async fn run(mut self) {
        let mut debounce_timer: Option<Pin<Box<Sleep>>> = None;
        let mut rate_limit_timer: Option<Pin<Box<Sleep>>> = None;
        let mut reconnect_timer: Option<Pin<Box<Sleep>>> = None;

        // Schedule immediate startup connection inside the select loop so Pause/Shutdown
        // commands remain responsive even during initial handshake attempts (DEF-06).
        if self.enabled && self.application_id.is_some() {
            reconnect_timer = Some(Box::pin(tokio::time::sleep(Duration::ZERO)));
        }

        loop {
            let mut next_cmd: Option<ServiceCommand> = None;
            tokio::select! {
                biased;

                // 1. Process incoming command (prioritized over expired timers on startup/shutdown)
                maybe_cmd = self.cmd_rx.recv() => {
                    let Some(cmd) = maybe_cmd else {
                        break;
                    };
                    next_cmd = Some(cmd);
                }

                // 2. Debounce timer expired (500 ms elapsed since last edit)
                _ = wait_optional_timer(&mut debounce_timer) => {
                    debounce_timer = None;
                    let (should_exit, deferred) = self.handle_debounce_expired(&mut rate_limit_timer, &mut reconnect_timer).await;
                    if should_exit {
                        break;
                    }
                    next_cmd = deferred;
                }

                // 3. Rate-limit delay timer expired (rolling window slot or Discord cooldown available)
                _ = wait_optional_timer(&mut rate_limit_timer) => {
                    rate_limit_timer = None;
                    let debounce_in_flight = debounce_timer.is_some();
                    let (should_exit, deferred) = self.handle_rate_limit_slot_available(&mut rate_limit_timer, &mut reconnect_timer, debounce_in_flight).await;
                    if should_exit {
                        break;
                    }
                    next_cmd = deferred;
                }

                // 4. Reconnect backoff timer (or startup connect trigger) expired
                _ = wait_optional_timer(&mut reconnect_timer) => {
                    reconnect_timer = None;
                    let (should_exit, deferred) = self.attempt_connect_and_publish(&mut debounce_timer, &mut rate_limit_timer, &mut reconnect_timer).await;
                    if should_exit {
                        break;
                    }
                    next_cmd = deferred;
                }

                // 5. Unsolicited event from adapter while connected
                evt = async {
                    if self.connected {
                        self.adapter.next_event().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    self.handle_adapter_event(evt, &mut rate_limit_timer, &mut reconnect_timer).await;
                }
            }

            while let Some(cmd) = next_cmd.take() {
                let (should_exit, deferred) = self
                    .process_command(
                        cmd,
                        &mut debounce_timer,
                        &mut rate_limit_timer,
                        &mut reconnect_timer,
                    )
                    .await;
                if should_exit {
                    return;
                }
                next_cmd = deferred;
            }
        }
    }

    /// Processes a `ServiceCommand`. Returns `(should_exit, optional_deferred_command)`.
    async fn process_command(
        &mut self,
        cmd: ServiceCommand,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        match cmd {
            ServiceCommand::Shutdown { completion } => {
                *debounce_timer = None;
                *rate_limit_timer = None;
                *reconnect_timer = None;
                self.handle_shutdown().await;
                let _ = completion.send(());
                (true, None)
            }
            ServiceCommand::SetActivity(activity) => {
                self.handle_set_activity(*activity, debounce_timer);
                (false, None)
            }
            ServiceCommand::ClearActivity => {
                self.handle_clear_activity(debounce_timer, rate_limit_timer, reconnect_timer)
                    .await;
                (false, None)
            }
            ServiceCommand::Pause => {
                self.handle_pause(debounce_timer, rate_limit_timer, reconnect_timer)
                    .await;
                (false, None)
            }
            ServiceCommand::Resume => {
                self.handle_resume(debounce_timer, rate_limit_timer, reconnect_timer)
                    .await
            }
            ServiceCommand::ResumeWithProfile { app_id, activity } => {
                self.handle_resume_with_profile(
                    app_id,
                    activity.map(|a| *a),
                    debounce_timer,
                    rate_limit_timer,
                    reconnect_timer,
                )
                .await
            }
            ServiceCommand::SetApplicationId(app_id) => {
                self.handle_set_app_id(app_id, debounce_timer, rate_limit_timer, reconnect_timer)
                    .await
            }
            ServiceCommand::ApplyProfile { app_id, activity } => {
                self.handle_apply_profile(
                    app_id,
                    *activity,
                    debounce_timer,
                    rate_limit_timer,
                    reconnect_timer,
                )
                .await
            }
            ServiceCommand::RetryNow => {
                self.handle_retry_now(debounce_timer, rate_limit_timer, reconnect_timer)
                    .await
            }
        }
    }

    fn handle_set_activity(
        &mut self,
        activity: PresenceActivity,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        // Latest edit replaces any pending snapshot ("latest wins")
        self.buffer.set(activity);
        // Start or reset trailing 500 ms debounce timer
        *debounce_timer = Some(Box::pin(tokio::time::sleep(DEBOUNCE_DURATION)));
        self.broadcast_status();
    }

    async fn handle_debounce_expired(
        &mut self,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        if !self.enabled || self.application_id.is_none() {
            return (false, None);
        }

        if !self.connected {
            let mut no_debounce = None;
            return self
                .attempt_connect_and_publish(&mut no_debounce, rate_limit_timer, reconnect_timer)
                .await;
        }

        self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
            .await;
        (false, None)
    }

    async fn handle_rate_limit_slot_available(
        &mut self,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
        debounce_in_flight: bool,
    ) -> (bool, Option<ServiceCommand>) {
        if self.buffer.has_pending() && !debounce_in_flight {
            if self.connected {
                self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
                    .await;
            } else if self.enabled && self.application_id.is_some() {
                let mut no_debounce = None;
                return self
                    .attempt_connect_and_publish(
                        &mut no_debounce,
                        rate_limit_timer,
                        reconnect_timer,
                    )
                    .await;
            }
        }
        (false, None)
    }

    fn resolve_activity_timestamps(&mut self, mut activity: PresenceActivity) -> PresenceActivity {
        let now_unix = self.current_unix_secs();
        match activity.timestamp_mode {
            TimestampMode::None => {
                activity.start_timestamp = None;
                activity.end_timestamp = None;
            }
            TimestampMode::SessionElapsed => {
                let start = *self.session_start_unix.get_or_insert(now_unix);
                activity.start_timestamp = Some(start);
                activity.end_timestamp = None;
            }
            TimestampMode::CountdownUntil => {
                activity.start_timestamp = None;
                if activity.end_timestamp.is_some_and(|end| end <= now_unix) {
                    activity.end_timestamp = None;
                }
            }
        }
        activity
    }

    async fn publish_pending_or_schedule(
        &mut self,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        if !self.enabled {
            return;
        }

        let Some(pending) = self.buffer.peek().cloned() else {
            return;
        };

        let now = Instant::now();
        if let Some(until) = self.rate_limit_until {
            if now < until {
                let padded_wait = (until - now).saturating_add(Duration::from_millis(10));
                *rate_limit_timer = Some(Box::pin(tokio::time::sleep(padded_wait)));
                self.broadcast_status();
                return;
            }
            self.rate_limit_until = None;
        }

        if self.rate_limiter.can_proceed(now) {
            let resolved = self.resolve_activity_timestamps(pending);
            match self.adapter.set_activity(&resolved).await {
                Ok(()) => {
                    let sent_at = Instant::now();
                    let _ = self.rate_limiter.record(sent_at);
                    let _ = self.buffer.take();
                    self.active_activity = Some(resolved);
                    self.state_machine.on_publish_success();
                    self.last_error = None;
                    self.broadcast_status();
                }
                Err(err) => {
                    self.handle_adapter_error(err, rate_limit_timer, reconnect_timer)
                        .await;
                }
            }
        } else if let Some(wait_time) = self.rate_limiter.time_until_available(now) {
            // Add 10ms defensive margin to guard against Windows timer scheduling jitter
            let padded_wait = wait_time.saturating_add(Duration::from_millis(10));
            *rate_limit_timer = Some(Box::pin(tokio::time::sleep(padded_wait)));
            self.broadcast_status();
        }
    }

    async fn handle_clear_activity(
        &mut self,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        *debounce_timer = None;
        *rate_limit_timer = None;
        self.buffer.clear();
        self.session_start_unix = None;

        if self.connected && self.active_activity.is_some() {
            match self.adapter.clear_activity().await {
                Ok(()) => {
                    self.active_activity = None;
                    self.last_error = None;
                    if self.state_machine.state() == PresenceState::Published {
                        self.state_machine.on_pipe_detected();
                    }
                }
                Err(err) => {
                    self.handle_clear_activity_error(err, reconnect_timer).await;
                }
            }
        } else {
            self.active_activity = None;
        }

        self.broadcast_status();
    }

    fn schedule_rate_limit_cooldown(
        &mut self,
        retry_after_secs: Option<u64>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> u64 {
        let cooldown_secs = retry_after_secs
            .unwrap_or(DEFAULT_RATE_LIMIT_RETRY_SECS)
            .max(1);
        let cooldown = Duration::from_secs(cooldown_secs);
        let now = Instant::now();
        let new_until = now + cooldown;
        let effective_until = self
            .rate_limit_until
            .map_or(new_until, |existing| existing.max(new_until));
        self.rate_limit_until = Some(effective_until);

        if self.enabled {
            let wait = effective_until.saturating_duration_since(now);
            *reconnect_timer = Some(Box::pin(tokio::time::sleep(wait)));
        }

        cooldown_secs
    }

    async fn disconnect_for_app_id_change(&mut self) {
        self.buffer.clear();
        self.active_activity = None;
        self.session_start_unix = None;

        if self.connected {
            let _ = self.adapter.clear_activity().await;
            let _ = self.adapter.disconnect().await;
            self.connected = false;
            self.session_info = None;
        }
    }

    async fn handle_clear_activity_error(
        &mut self,
        err: DiscordError,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        let clear_msg = err.clear_failure_message().to_owned();
        match err {
            DiscordError::RateLimited { retry_after_secs } => {
                let cooldown_secs =
                    self.schedule_rate_limit_cooldown(retry_after_secs, reconnect_timer);

                let _ = self.adapter.disconnect().await;
                self.connected = false;
                self.session_info = None;
                self.active_activity = None;
                self.last_error = Some(clear_msg);

                let normalized_err = DiscordError::RateLimited {
                    retry_after_secs: Some(cooldown_secs),
                };
                let _ = self.state_machine.on_disconnect(&normalized_err);
            }
            DiscordError::ClearTimeout
            | DiscordError::Disconnected
            | DiscordError::NotConnected
            | DiscordError::DiscordNotRunning
            | DiscordError::PipeBusy
            | DiscordError::HandshakeTimeout
            | DiscordError::Io(_) => {
                let _ = self.adapter.disconnect().await;
                self.connected = false;
                self.session_info = None;
                self.active_activity = None;
                self.last_error = Some(clear_msg);

                let (_new_state, delay) = self.state_machine.on_disconnect(&err);
                if delay > Duration::ZERO && self.enabled {
                    *reconnect_timer = Some(Box::pin(tokio::time::sleep(delay)));
                }
            }
            DiscordError::RpcRejected { .. }
            | DiscordError::ProtocolViolation
            | DiscordError::ProtocolMismatch { .. }
            | DiscordError::InvalidClientId => {
                // Pipe stays open and Discord rejected CLEAR_ACTIVITY: keep active_activity
                // instead of falsely reporting that the activity was cleared.
                self.last_error = Some(clear_msg);
            }
        }
    }

    async fn handle_pause(
        &mut self,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        self.enabled = false;
        *debounce_timer = None;
        *rate_limit_timer = None;
        *reconnect_timer = None;
        self.buffer.clear();
        self.session_start_unix = None;

        if self.connected {
            let clear_res = self.adapter.clear_activity().await;
            // Always run best-effort disconnect teardown even if clear fails; a disconnect error
            // after an acknowledged clear MUST NOT be reported as an unconfirmed activity clear.
            let _disconnect_res = self.adapter.disconnect().await;
            self.connected = false;
            self.session_info = None;
            self.active_activity = None;
            self.last_error = clear_res
                .err()
                .map(|err| err.clear_failure_message().to_owned());
        } else {
            self.active_activity = None;
            self.last_error = None;
        }

        self.state_machine.on_user_pause();
        self.broadcast_status();
    }

    async fn handle_resume(
        &mut self,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        if self.enabled && self.connected {
            return (false, None);
        }

        self.enabled = true;
        self.state_machine.on_user_resume();
        self.last_error = None;

        if self.application_id.is_some() {
            let (should_exit, deferred) = self
                .attempt_connect_and_publish(debounce_timer, rate_limit_timer, reconnect_timer)
                .await;
            if should_exit || deferred.is_some() {
                return (should_exit, deferred);
            }
        } else {
            self.state_machine.on_user_pause();
        }

        self.broadcast_status();
        (false, None)
    }

    async fn handle_resume_with_profile(
        &mut self,
        app_id: Option<ApplicationId>,
        activity: Option<PresenceActivity>,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        *debounce_timer = None;
        *rate_limit_timer = None;
        *reconnect_timer = None;
        self.last_error = None;

        let app_id_changed = self.application_id != app_id;
        if app_id_changed {
            self.disconnect_for_app_id_change().await;
            self.application_id = app_id;
        }

        if let Some(act) = activity {
            self.buffer.set(act);
        }

        self.enabled = true;
        if self.application_id.is_none() {
            self.state_machine.on_user_pause();
            self.broadcast_status();
            return (false, None);
        }

        if self.connected {
            if self.buffer.has_pending() {
                self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
                    .await;
            }
            self.broadcast_status();
            return (false, None);
        }

        self.state_machine.on_user_resume();
        let (should_exit, deferred) = self
            .attempt_connect_and_publish(debounce_timer, rate_limit_timer, reconnect_timer)
            .await;
        if !should_exit && deferred.is_none() {
            self.broadcast_status();
        }
        (should_exit, deferred)
    }

    async fn handle_retry_now(
        &mut self,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        *reconnect_timer = None;
        self.rate_limit_until = None;
        self.state_machine.on_retry_now();
        self.last_error = None;

        if self.enabled && self.application_id.is_some() {
            if self.connected {
                if !self.buffer.has_pending() {
                    if let Some(active) = self.active_activity.clone() {
                        self.buffer.set(active);
                    }
                }
                if self.buffer.has_pending() {
                    self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
                        .await;
                } else {
                    self.broadcast_status();
                }
                (false, None)
            } else {
                self.attempt_connect_and_publish(debounce_timer, rate_limit_timer, reconnect_timer)
                    .await
            }
        } else {
            self.broadcast_status();
            (false, None)
        }
    }

    async fn handle_set_app_id(
        &mut self,
        new_app_id: Option<ApplicationId>,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        if self.application_id == new_app_id {
            return (false, None);
        }

        // Cancel all in-flight timers and clear activity belonging to the old App ID (DEF-04)
        *debounce_timer = None;
        *rate_limit_timer = None;
        *reconnect_timer = None;
        self.rate_limit_until = None;
        self.disconnect_for_app_id_change().await;

        self.application_id = new_app_id;

        if self.application_id.is_some() && self.enabled {
            self.state_machine.on_user_resume();
            return self
                .attempt_connect_and_publish(debounce_timer, rate_limit_timer, reconnect_timer)
                .await;
        }

        self.state_machine.on_user_pause();
        self.broadcast_status();
        (false, None)
    }

    async fn handle_apply_profile(
        &mut self,
        new_app_id: Option<ApplicationId>,
        activity: PresenceActivity,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        let app_id_changed = self.application_id != new_app_id;

        if app_id_changed {
            *debounce_timer = None;
            *rate_limit_timer = None;
            *reconnect_timer = None;
            self.rate_limit_until = None;
            self.disconnect_for_app_id_change().await;
            self.application_id = new_app_id;
        } else if self.connected
            && self.state_machine.state() == PresenceState::Published
            && !self.buffer.has_pending()
        {
            let mut normalized_for_dedup = activity.clone();
            if normalized_for_dedup.timestamp_mode == TimestampMode::CountdownUntil {
                let now_unix = self.current_unix_secs();
                if normalized_for_dedup
                    .end_timestamp
                    .is_some_and(|end| end <= now_unix)
                {
                    normalized_for_dedup.end_timestamp = None;
                }
            }
            if self.active_activity.as_ref().is_some_and(|active| {
                activities_equivalent_for_dedup(active, &normalized_for_dedup)
            }) {
                // Avoid unnecessary duplicate SET_ACTIVITY when App ID and resolved activity are unchanged (DEF-04)
                return (false, None);
            }
        }

        if self.application_id.is_none() {
            self.buffer.clear();
            self.active_activity = None;
            self.session_start_unix = None;
            self.state_machine.on_user_pause();
            self.broadcast_status();
            return (false, None);
        }

        *debounce_timer = None;
        if activity.timestamp_mode != TimestampMode::SessionElapsed {
            self.session_start_unix = None;
        }
        self.buffer.set(activity);

        if !self.enabled {
            self.broadcast_status();
            return (false, None);
        }

        if self.connected {
            self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
                .await;
            return (false, None);
        }

        if app_id_changed {
            self.state_machine.on_user_resume();
        }
        self.attempt_connect_and_publish(debounce_timer, rate_limit_timer, reconnect_timer)
            .await
    }

    /// Connects to Discord while keeping `cmd_rx` responsive to `Pause`, `Shutdown`, and updates.
    /// Returns `(should_exit, optional_deferred_command)`.
    async fn attempt_connect_and_publish(
        &mut self,
        debounce_timer: &mut Option<Pin<Box<Sleep>>>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) -> (bool, Option<ServiceCommand>) {
        let Some(app_id) = self.application_id.clone() else {
            self.state_machine.on_user_pause();
            self.broadcast_status();
            return (false, None);
        };

        if !self.enabled {
            return (false, None);
        }

        let now = Instant::now();
        if let Some(until) = self.rate_limit_until {
            if now < until {
                let wait = (until - now).saturating_add(Duration::from_millis(10));
                *reconnect_timer = Some(Box::pin(tokio::time::sleep(wait)));
                self.broadcast_status();
                return (false, None);
            }
            self.rate_limit_until = None;
        }

        self.state_machine.on_pipe_detected();
        self.broadcast_status();

        enum ConnectOutcome {
            Connected(Result<SessionInfo, DiscordError>),
            Interrupted(ServiceCommand),
            ChannelClosed,
        }

        let outcome = {
            let connect_fut = self.adapter.connect(&app_id);
            tokio::pin!(connect_fut);

            loop {
                tokio::select! {
                    biased;

                    maybe_cmd = self.cmd_rx.recv() => {
                        match maybe_cmd {
                            None => break ConnectOutcome::ChannelClosed,
                            Some(cmd @ (
                                ServiceCommand::Shutdown { .. }
                                | ServiceCommand::Pause
                                | ServiceCommand::SetApplicationId(_)
                                | ServiceCommand::ApplyProfile { .. }
                                | ServiceCommand::ResumeWithProfile { .. }
                            )) => {
                                break ConnectOutcome::Interrupted(cmd);
                            }
                            Some(ServiceCommand::SetActivity(activity)) => {
                                self.buffer.set(*activity);
                                *debounce_timer =
                                    Some(Box::pin(tokio::time::sleep(DEBOUNCE_DURATION)));
                                broadcast_status_snapshot(
                                    &self.status_tx,
                                    &self.state_machine,
                                    self.application_id.as_ref(),
                                    self.connected,
                                    &self.active_activity,
                                    &self.buffer,
                                    self.session_info.as_ref(),
                                    self.last_error.as_deref(),
                                );
                            }
                            Some(ServiceCommand::ClearActivity) => {
                                *debounce_timer = None;
                                *rate_limit_timer = None;
                                self.buffer.clear();
                                self.active_activity = None;
                                self.session_start_unix = None;
                                broadcast_status_snapshot(
                                    &self.status_tx,
                                    &self.state_machine,
                                    self.application_id.as_ref(),
                                    self.connected,
                                    &self.active_activity,
                                    &self.buffer,
                                    self.session_info.as_ref(),
                                    self.last_error.as_deref(),
                                );
                            }
                            Some(ServiceCommand::Resume | ServiceCommand::RetryNow) => {
                                // Already connecting with the current App ID
                            }
                        }
                    }
                    res = &mut connect_fut => {
                        break ConnectOutcome::Connected(res);
                    }
                }
            }
        };

        match outcome {
            ConnectOutcome::ChannelClosed => (true, None),
            ConnectOutcome::Interrupted(cmd) => {
                // Ensure any partial connection is cleaned up before processing interrupting command
                let _ = self.adapter.disconnect().await;
                self.connected = false;
                self.session_info = None;
                (false, Some(cmd))
            }
            ConnectOutcome::Connected(Ok(session)) => {
                self.connected = true;
                self.session_info = Some(session);
                self.last_error = None;

                // If no debounce is currently ticking, publish pending (or restore active)
                if debounce_timer.is_none() {
                    if !self.buffer.has_pending() {
                        if let Some(active) = self.active_activity.clone() {
                            self.buffer.set(active);
                        }
                    }

                    if self.buffer.has_pending() {
                        self.publish_pending_or_schedule(rate_limit_timer, reconnect_timer)
                            .await;
                    }
                    // Note (DEF-04): when both buffer and active_activity are None,
                    // do NOT transition to Published; remain in Connecting until SET_ACTIVITY ACKs.
                }

                self.broadcast_status();
                (false, None)
            }
            ConnectOutcome::Connected(Err(err)) => {
                self.handle_adapter_error(err, rate_limit_timer, reconnect_timer)
                    .await;
                (false, None)
            }
        }
    }

    async fn handle_adapter_error(
        &mut self,
        err: DiscordError,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        if self.connected {
            let _ = self.adapter.disconnect().await;
        }
        self.connected = false;
        self.session_info = None;
        self.session_start_unix = None;
        self.last_error = Some(err.user_message().to_owned());

        if let DiscordError::RateLimited { retry_after_secs } = err {
            *rate_limit_timer = None;
            let _ = self.schedule_rate_limit_cooldown(retry_after_secs, reconnect_timer);
            let _ = self.state_machine.on_disconnect(&err);
            self.broadcast_status();
            return;
        }

        let (_new_state, delay) = self.state_machine.on_disconnect(&err);
        if delay > Duration::ZERO && self.enabled {
            *reconnect_timer = Some(Box::pin(tokio::time::sleep(delay)));
        }

        self.broadcast_status();
    }

    async fn handle_adapter_event(
        &mut self,
        evt: AdapterEvent,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        match evt {
            AdapterEvent::Disconnected { reason } => {
                self.handle_adapter_error(reason, rate_limit_timer, reconnect_timer)
                    .await;
            }
            AdapterEvent::RpcError { code, message: _ } => {
                if code == 4002 {
                    self.handle_rate_limited_event(None, rate_limit_timer, reconnect_timer)
                        .await;
                    return;
                }

                let mapped_err = match code {
                    4000 => DiscordError::InvalidClientId,
                    4004 => DiscordError::ProtocolMismatch { version: 0 },
                    other => DiscordError::RpcRejected {
                        code: other,
                        message: String::new(),
                    },
                };

                self.connected = false;
                self.session_info = None;
                self.session_start_unix = None;
                self.active_activity = None;
                // Sanitized user-safe error only; never leak raw peer-supplied RPC message (DEF-05)
                self.last_error = Some(mapped_err.user_message().to_owned());
                let _ = self.adapter.disconnect().await;
                let _ = self.state_machine.on_disconnect(&mapped_err);
                self.broadcast_status();
            }
            AdapterEvent::RateLimited { retry_after_secs } => {
                self.handle_rate_limited_event(retry_after_secs, rate_limit_timer, reconnect_timer)
                    .await;
            }
        }
    }

    async fn handle_rate_limited_event(
        &mut self,
        retry_after_secs: Option<u64>,
        rate_limit_timer: &mut Option<Pin<Box<Sleep>>>,
        reconnect_timer: &mut Option<Pin<Box<Sleep>>>,
    ) {
        let cooldown_secs = self.schedule_rate_limit_cooldown(retry_after_secs, reconnect_timer);
        *rate_limit_timer = None;

        // Re-stage active activity so it is republished cleanly after cooldown
        if !self.buffer.has_pending() {
            if let Some(active) = self.active_activity.take() {
                self.buffer.set(active);
            }
        } else {
            self.active_activity = None;
        }
        self.session_start_unix = None;

        if self.connected {
            let _ = self.adapter.disconnect().await;
            self.connected = false;
            self.session_info = None;
        }

        let err = DiscordError::RateLimited {
            retry_after_secs: Some(cooldown_secs),
        };
        self.last_error = Some(err.user_message().to_owned());
        let _ = self.state_machine.on_disconnect(&err);
        self.broadcast_status();
    }

    async fn handle_shutdown(&mut self) {
        self.enabled = false;
        self.state_machine.on_exit();
        self.buffer.clear();
        self.active_activity = None;
        self.session_start_unix = None;

        if self.connected {
            let _ =
                tokio::time::timeout(Duration::from_millis(1000), self.adapter.clear_activity())
                    .await;
            let _ =
                tokio::time::timeout(Duration::from_millis(1000), self.adapter.disconnect()).await;
            self.connected = false;
            self.session_info = None;
        }

        self.broadcast_status();
    }
}

async fn wait_optional_timer(timer: &mut Option<Pin<Box<Sleep>>>) {
    match timer {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending().await,
    }
}

#[allow(clippy::too_many_arguments)]
fn broadcast_status_snapshot(
    status_tx: &watch::Sender<PresenceServiceStatus>,
    state_machine: &PresenceReconnectStateMachine,
    application_id: Option<&ApplicationId>,
    connected: bool,
    active_activity: &Option<PresenceActivity>,
    buffer: &CoalescingBuffer<PresenceActivity>,
    session_info: Option<&SessionInfo>,
    last_error: Option<&str>,
) {
    let status = PresenceServiceStatus {
        state: state_machine.state(),
        status_label: state_machine.state().label(),
        masked_app_id: application_id.map(ApplicationId::masked),
        is_connected: connected,
        active_activity: active_activity.clone(),
        pending_activity: buffer.peek().cloned(),
        pipe_slot: session_info.map(|s| s.pipe_slot),
        protocol_version: session_info.map(|s| s.protocol_version),
        consecutive_failures: state_machine.consecutive_failures(),
        last_error: last_error.map(str::to_owned),
    };
    let _ = status_tx.send(status);
}

fn activities_equivalent_for_dedup(a: &PresenceActivity, b: &PresenceActivity) -> bool {
    a.activity_type == b.activity_type
        && a.status_display_type == b.status_display_type
        && a.name_override == b.name_override
        && a.details == b.details
        && a.details_url == b.details_url
        && a.state == b.state
        && a.state_url == b.state_url
        && a.large_asset == b.large_asset
        && a.large_text == b.large_text
        && a.large_url == b.large_url
        && a.small_asset == b.small_asset
        && a.small_text == b.small_text
        && a.small_url == b.small_url
        && a.timestamp_mode == b.timestamp_mode
        && a.buttons == b.buttons
        && match a.timestamp_mode {
            TimestampMode::None | TimestampMode::SessionElapsed => true,
            TimestampMode::CountdownUntil => a.end_timestamp == b.end_timestamp,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::MockDiscordAdapter;
    use crate::presence::{ActivityType, ArtworkRef, StatusDisplayType, TimestampMode};

    fn make_test_activity(detail: &str) -> PresenceActivity {
        PresenceActivity {
            activity_type: ActivityType::Playing,
            status_display_type: Some(StatusDisplayType::Name),
            name_override: None,
            details: Some(detail.to_owned()),
            details_url: None,
            state: Some("Testing".to_owned()),
            state_url: None,
            large_asset: ArtworkRef::None,
            large_text: None,
            large_url: None,
            small_asset: ArtworkRef::None,
            small_text: None,
            small_url: None,
            timestamp_mode: TimestampMode::None,
            start_timestamp: None,
            end_timestamp: None,
            buttons: Vec::new(),
        }
    }

    #[tokio::test]
    async fn service_connects_and_publishes_after_debounce(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::new(adapter, Some(app_id), true);
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        // Wait for initial connect (with no activity, must NOT report Published per DEF-04)
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }
        assert_eq!(rx.borrow().state, PresenceState::Connecting);
        assert_eq!(rx.borrow().active_activity, None);

        // Submit activity
        handle.set_activity(make_test_activity("V1")).await?;
        rx.changed().await?;
        assert_eq!(rx.borrow().active_activity, None);
        assert!(rx.borrow().pending_activity.is_some());

        // Wait for 500 ms debounce to expire and publish
        while rx.borrow().active_activity.is_none() {
            rx.changed().await?;
        }
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Published);
        assert_eq!(
            status
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("V1")
        );
        assert_eq!(status.pending_activity, None);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_debounce_ensures_latest_activity_wins(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::new(adapter, Some(app_id), true);
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }

        // Rapidly dispatch 3 edits within debounce window
        handle.set_activity(make_test_activity("Edit 1")).await?;
        rx.changed().await?;
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle.set_activity(make_test_activity("Edit 2")).await?;
        rx.changed().await?;
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle.set_activity(make_test_activity("Edit 3")).await?;
        rx.changed().await?;

        // Prior to debounce completion, Edit 3 must be pending
        assert_eq!(rx.borrow().active_activity, None);
        assert_eq!(
            rx.borrow()
                .pending_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("Edit 3")
        );

        // Wait for trailing debounce to expire
        while rx.borrow().active_activity.is_none() {
            rx.changed().await?;
        }
        let status = rx.borrow().clone();
        assert_eq!(
            status
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("Edit 3")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_clear_activity_cancels_pending_debounced_update(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::new(adapter, Some(app_id), true);
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }

        handle
            .set_activity(make_test_activity("WillCancel"))
            .await?;
        rx.changed().await?;
        assert!(rx.borrow().pending_activity.is_some());

        // Immediately clear
        handle.clear_activity().await?;
        rx.changed().await?;
        assert_eq!(rx.borrow().pending_activity, None);

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(rx.borrow().active_activity, None);
        assert_ne!(rx.borrow().state, PresenceState::Published);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_handles_disconnect_and_reconnect_backoff(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(handle.status().state, PresenceState::Published);

        // Inject pipe disconnect event
        event_tx
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;

        tokio::time::sleep(Duration::from_millis(50)).await;
        let status = handle.status();
        assert!(!status.is_connected);
        assert_eq!(status.state, PresenceState::Reconnecting);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_non_retryable_error_stops_retry_loop() -> Result<(), Box<dyn std::error::Error>>
    {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Inject non-retryable 4000 Invalid Client ID error
        event_tx
            .send(AdapterEvent::RpcError {
                code: 4000,
                message: "Invalid Client ID".to_owned(),
            })
            .await?;

        tokio::time::sleep(Duration::from_millis(50)).await;
        let status = handle.status();
        assert_eq!(status.state, PresenceState::Error);
        assert!(!status.is_connected);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_pause_and_resume_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            adapter,
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert!(rx.borrow().is_connected);

        // Pause must clear active_activity and disconnect
        handle.pause().await?;
        while rx.borrow().state != PresenceState::Disabled {
            rx.changed().await?;
        }
        assert!(!rx.borrow().is_connected);
        assert_eq!(rx.borrow().active_activity, None);

        // Resume with active profile restores Published state
        handle
            .resume_with_profile(Some(app_id), Some(make_test_activity("Restored")))
            .await?;
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert!(rx.borrow().is_connected);
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("Restored")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_set_application_id_switches_session() -> Result<(), Box<dyn std::error::Error>>
    {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id_1 = ApplicationId::parse("111111111111111111")?;
        let app_id_2 = ApplicationId::parse("222222222222222222")?;

        let (service, handle) = PresenceService::with_initial_activity(
            adapter,
            Some(app_id_1),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert_eq!(rx.borrow().masked_app_id, Some("****1111".to_owned()));

        // Switch to second Application ID
        handle.set_application_id(Some(app_id_2)).await?;
        while rx.borrow().masked_app_id.as_deref() != Some("****2222") {
            rx.changed().await?;
        }
        // Old App ID's activity must be cleared on App ID change
        assert_eq!(rx.borrow().active_activity, None);
        assert_ne!(rx.borrow().state, PresenceState::Published);

        // Clear Application ID (transitions to Disabled)
        handle.set_application_id(None).await?;
        while rx.borrow().state != PresenceState::Disabled {
            rx.changed().await?;
        }
        assert_eq!(rx.borrow().masked_app_id, None);
        assert!(!rx.borrow().is_connected);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_rate_limiter_throttles_excess_and_publishes_latest(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (mut service, handle) = PresenceService::new(adapter, Some(app_id), true);
        // Configure a fast 1-second rolling window with max 2 updates for deterministic fast testing
        service.rate_limiter = RollingRateLimiter::new(2, Duration::from_secs(1));
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }

        // 1st update
        handle.set_activity(make_test_activity("Slot 1")).await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Slot 1")
        {
            rx.changed().await?;
        }

        // 2nd update
        handle.set_activity(make_test_activity("Slot 2")).await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Slot 2")
        {
            rx.changed().await?;
        }

        // 3rd & 4th updates dispatched while rate limited: 4th must supersede 3rd
        handle
            .set_activity(make_test_activity("Slot 3 (superseded)"))
            .await?;
        rx.changed().await?;
        handle
            .set_activity(make_test_activity("Slot 4 (winner)"))
            .await?;
        rx.changed().await?;

        // Wait for rate limit window to reopen and publish the newest snapshot
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Slot 4 (winner)")
        {
            rx.changed().await?;
        }

        let status = rx.borrow().clone();
        assert_eq!(
            status
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("Slot 4 (winner)")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_reconnect_under_active_rate_limit_schedules_publish_and_restores_activity(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (mut service, handle) = PresenceService::new(Box::new(mock), Some(app_id), true);
        // Fast 1-second rolling limiter with max 1 update
        service.rate_limiter = RollingRateLimiter::new(1, Duration::from_secs(1));
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }

        // Publish activity #1
        handle
            .set_activity(make_test_activity("PersistAcrossReconnect"))
            .await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("PersistAcrossReconnect")
        {
            rx.changed().await?;
        }

        // Now rate limiter is full (1 update used within 1s).
        // Disconnect immediately.
        event_tx
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;

        while rx.borrow().is_connected {
            rx.changed().await?;
        }
        assert_eq!(rx.borrow().state, PresenceState::Reconnecting);

        // Retry immediately while the 1s rate limit window is still active
        handle.retry_now().await?;

        // Reconnect succeeds; service detects rate limit cooldown, schedules timer,
        // and once the cooldown expires, restores the active activity on Discord
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("PersistAcrossReconnect")
            || rx.borrow().state != PresenceState::Published
        {
            rx.changed().await?;
        }

        let final_status = rx.borrow().clone();
        assert!(final_status.is_connected);
        assert_eq!(final_status.state, PresenceState::Published);
        assert_eq!(
            final_status
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("PersistAcrossReconnect")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_debounce_in_flight_during_connect_delays_publish_until_debounce_finishes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        // Start disabled so no connection occurs initially
        let (service, handle) = PresenceService::new(adapter, Some(app_id), false);
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        assert_eq!(rx.borrow().state, PresenceState::Disabled);

        // User edits activity while disabled/offline
        handle
            .set_activity(make_test_activity("DebounceOverConnect"))
            .await?;
        rx.changed().await?;
        assert!(rx.borrow().pending_activity.is_some());

        // Resume after 50 ms (debounce timer has 450 ms remaining)
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.resume().await?;

        // Connection succeeds quickly, but activity must NOT be published yet because debounce is running
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mid_status = rx.borrow().clone();
        assert!(mid_status.is_connected);
        assert_eq!(mid_status.active_activity, None);
        assert!(mid_status.pending_activity.is_some());

        // Wait for trailing 500 ms debounce to finish and publish
        while rx.borrow().active_activity.is_none() {
            rx.changed().await?;
        }

        let final_status = rx.borrow().clone();
        assert_eq!(final_status.state, PresenceState::Published);
        assert_eq!(
            final_status
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("DebounceOverConnect")
        );
        assert_eq!(final_status.pending_activity, None);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_clearing_application_id_cancels_all_timers_and_clears_active_presence(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::new(adapter, Some(app_id), true);
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while !rx.borrow().is_connected {
            rx.changed().await?;
        }

        handle
            .set_activity(make_test_activity("ActiveBeforeClearAppId"))
            .await?;
        while rx.borrow().active_activity.is_none() {
            rx.changed().await?;
        }

        // Clearing App ID must disconnect, clear active_activity, clear buffer, and set state to Disabled
        handle.set_application_id(None).await?;
        while rx.borrow().state != PresenceState::Disabled {
            rx.changed().await?;
        }

        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert_eq!(status.active_activity, None);
        assert_eq!(status.pending_activity, None);
        assert_eq!(status.masked_app_id, None);
        assert!(!status.is_connected);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_rapid_consecutive_clear_activity_is_idempotent(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            adapter,
            Some(app_id),
            true,
            Some(make_test_activity("ActiveBeforeClear")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }

        // Multiple rapid clear calls while active should succeed idempotently
        handle.clear_activity().await?;
        handle.clear_activity().await?;
        handle.clear_activity().await?;

        while rx.borrow().active_activity.is_some() {
            rx.changed().await?;
        }
        let status = rx.borrow().clone();
        assert_eq!(status.active_activity, None);
        assert_eq!(status.pending_activity, None);
        assert_ne!(status.state, PresenceState::Published);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn service_shutdown_when_already_disconnected_completes_immediately(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let adapter = Box::new(MockDiscordAdapter::new());
        // Initialized without App ID (disconnected / disabled)
        let (service, handle) = PresenceService::new(adapter, None, false);
        tokio::spawn(service.run());

        let start = std::time::Instant::now();
        // Shutdown on disconnected service must complete immediately (< 100ms) without timeouts
        handle.shutdown().await?;
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        Ok(())
    }

    #[tokio::test]
    async fn def01_pause_clears_active_activity_cancels_debounce_and_is_idempotent(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("PublishedBeforePause")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        // Queue a pending debounced edit, then immediately Pause before 500 ms expires
        handle
            .set_activity(make_test_activity("StaleQueuedBeforePause"))
            .await?;
        tokio::time::timeout(wait_limit, rx.wait_for(|s| s.pending_activity.is_some()))
            .await
            .map_err(|_| "timed out waiting for pending debounced activity")??;

        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled on first pause")??;

        handle.pause().await?; // Repeated pause must be idempotent
        tokio::time::timeout(wait_limit, rx.changed())
            .await
            .map_err(|_| "timed out waiting for broadcast on second pause")??;

        // Confirm longer than 500 ms debounce produces zero spurious broadcasts or publishes after Pause
        assert!(
            tokio::time::timeout(Duration::from_millis(700), rx.changed())
                .await
                .is_err(),
            "unexpected status update during post-Pause debounce check"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert_eq!(status.active_activity, None);
        assert_eq!(status.pending_activity, None);
        assert_eq!(status.last_error, None);
        assert!(!status.is_connected);

        let recorded_after_pause = calls.lock().map_err(|_| "lock poisoned")?.clone();
        let clear_indices: Vec<_> = recorded_after_pause
            .iter()
            .enumerate()
            .filter_map(|(idx, c)| matches!(c, MockAdapterCall::ClearActivity).then_some(idx))
            .collect();
        let disconnect_indices: Vec<_> = recorded_after_pause
            .iter()
            .enumerate()
            .filter_map(|(idx, c)| matches!(c, MockAdapterCall::Disconnect).then_some(idx))
            .collect();
        assert_eq!(clear_indices.len(), 1);
        assert_eq!(disconnect_indices.len(), 1);
        assert_eq!(disconnect_indices[0], clear_indices[0] + 1);
        assert!(!recorded_after_pause.iter().any(|c| matches!(
            c,
            MockAdapterCall::SetActivity { activity }
                if activity.details.as_deref() == Some("StaleQueuedBeforePause")
        )));

        // Resume with profile and repeat resume idempotently
        handle
            .resume_with_profile(Some(app_id), Some(make_test_activity("ResumedProfile")))
            .await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published after resume")??;
        handle.resume().await?; // Idempotent when already connected & enabled
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("ResumedProfile")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def04_apply_profile_prevents_cross_app_leakage_and_duplicate_publishes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_1 = ApplicationId::parse("111111111111111111")?;
        let app_2 = ApplicationId::parse("222222222222222222")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_1.clone()),
            true,
            Some(make_test_activity("Profile A")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }

        // Applying identical profile + App ID must NOT send a duplicate SET_ACTIVITY
        handle
            .apply_profile(Some(app_1.clone()), make_test_activity("Profile A"))
            .await?;
        tokio::time::sleep(Duration::from_millis(40)).await;

        let set_count_before = calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .iter()
            .filter(|c| matches!(c, MockAdapterCall::SetActivity { .. }))
            .count();
        assert_eq!(set_count_before, 1);

        // Switch atomically to Profile B with App ID 2
        handle
            .apply_profile(Some(app_2), make_test_activity("Profile B"))
            .await?;
        while rx.borrow().masked_app_id.as_deref() != Some("****2222")
            || rx
                .borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref())
                != Some("Profile B")
        {
            rx.changed().await?;
        }

        // Verify call log: Profile A was NEVER published after Connect(****2222)
        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        let connect_2_idx = recorded
            .iter()
            .position(|c| {
                matches!(
                    c,
                    MockAdapterCall::Connect { masked_app_id } if masked_app_id == "****2222"
                )
            })
            .ok_or("missing Connect for app 2")?;
        let post_switch_sets: Vec<_> = recorded[connect_2_idx..]
            .iter()
            .filter_map(|c| match c {
                MockAdapterCall::SetActivity { activity } => activity.details.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(post_switch_sets, vec!["Profile B"]);

        // Queue old buffered edit on App ID 2, then immediately ApplyProfile to App ID 1:
        // buffered edit for App ID 2 must be discarded and never published under App ID 1.
        handle
            .set_activity(make_test_activity("StaleBufferedApp2"))
            .await?;
        handle
            .apply_profile(Some(app_1), make_test_activity("CleanApp1"))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while rx.borrow().masked_app_id.as_deref() != Some("****1111")
                || rx
                    .borrow()
                    .active_activity
                    .as_ref()
                    .and_then(|a| a.details.as_deref())
                    != Some("CleanApp1")
            {
                rx.changed().await?;
            }
            Ok::<(), tokio::sync::watch::error::RecvError>(())
        })
        .await??;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref()),
            Some("CleanApp1")
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def05_rate_limit_enforces_cooldown_and_sanitizes_rpc_errors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }

        // 1. Rate-limit event with explicit 1s retry_after_secs blocks publishing during cooldown and recovers
        event_tx
            .send(AdapterEvent::RateLimited {
                retry_after_secs: Some(1),
            })
            .await?;

        while rx.borrow().state != PresenceState::Reconnecting {
            rx.changed().await?;
        }
        assert!(!rx.borrow().is_connected);

        // Sending an edit during cooldown must NOT publish immediately
        handle
            .set_activity(make_test_activity("AfterCooldown"))
            .await?;
        tokio::time::sleep(Duration::from_millis(600)).await;
        let during_cooldown_sets = calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .iter()
            .filter(|c| match c {
                MockAdapterCall::SetActivity { activity } => {
                    activity.details.as_deref() == Some("AfterCooldown")
                }
                _ => false,
            })
            .count();
        assert_eq!(during_cooldown_sets, 0);

        // After 1s cooldown expires, it recovers and publishes "AfterCooldown"
        while rx.borrow().state != PresenceState::Published
            || rx
                .borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.details.as_deref())
                != Some("AfterCooldown")
        {
            rx.changed().await?;
        }
        assert_eq!(rx.borrow().last_error, None);

        // 2. Unsolicited RpcError with unknown code 9999 must sanitize peer message and enter Error
        event_tx
            .send(AdapterEvent::RpcError {
                code: 9999,
                message: "RAW_SENSITIVE_PEER_MESSAGE_C:\\Users\\Alice\\secret".to_owned(),
            })
            .await?;
        while rx.borrow().state != PresenceState::Error {
            rx.changed().await?;
        }
        let err_msg = rx
            .borrow()
            .last_error
            .clone()
            .ok_or("expected sanitized error message")?;
        assert!(!err_msg.contains("RAW_SENSITIVE_PEER_MESSAGE"));
        assert!(!err_msg.contains("Alice"));

        // 3. Unsolicited 4004 ProtocolMismatch also maps cleanly to Error without raw peer message
        handle
            .apply_profile(Some(app_id), make_test_activity("RecoveredAfterRpcError"))
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while rx.borrow().state != PresenceState::Published {
                rx.changed().await?;
            }
            Ok::<(), tokio::sync::watch::error::RecvError>(())
        })
        .await??;
        event_tx
            .send(AdapterEvent::RpcError {
                code: 4004,
                message: "RAW_4004_MESSAGE".to_owned(),
            })
            .await?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while rx.borrow().state != PresenceState::Error {
                rx.changed().await?;
            }
            Ok::<(), tokio::sync::watch::error::RecvError>(())
        })
        .await??;
        let msg_4004 = rx.borrow().last_error.clone().unwrap_or_default();
        assert!(!msg_4004.contains("RAW_4004_MESSAGE"));

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def05_rate_limit_without_retry_duration_uses_fallback_cooldown(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Initial")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }

        event_tx
            .send(AdapterEvent::RateLimited {
                retry_after_secs: None,
            })
            .await?;
        while rx.borrow().state != PresenceState::Reconnecting {
            rx.changed().await?;
        }

        // Edit during fallback 25s cooldown must NOT publish
        handle
            .set_activity(make_test_activity("BlockedDuringFallback"))
            .await?;
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert_ne!(rx.borrow().state, PresenceState::Published);
        assert_eq!(rx.borrow().active_activity, None);

        // Repeated rate-limit event while already cooling down must keep blocking publication
        event_tx
            .send(AdapterEvent::RateLimited {
                retry_after_secs: Some(30),
            })
            .await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_ne!(rx.borrow().state, PresenceState::Published);
        assert_eq!(rx.borrow().active_activity, None);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def06_startup_stages_initial_activity_and_keeps_pause_shutdown_responsive(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        // 1. Offline Discord at startup triggers only ONE connect attempt (no duplicate race)
        let mut offline_mock = MockDiscordAdapter::new();
        let offline_calls = offline_mock.shared_calls();
        offline_mock.push_connect_result(Err(DiscordError::DiscordNotRunning));
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(offline_mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("StartupActivity")),
        );
        tokio::spawn(service.run());

        tokio::time::sleep(Duration::from_millis(150)).await;
        let connect_attempts = offline_calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .iter()
            .filter(|c| matches!(c, MockAdapterCall::Connect { .. }))
            .count();
        assert_eq!(connect_attempts, 1);
        handle.shutdown().await?;

        // 2. Pause and Shutdown during slow connect() return immediately without waiting for 2s connect_delay
        let mut slow_mock = MockDiscordAdapter::new();
        slow_mock.set_connect_delay(Duration::from_secs(2));
        let (slow_svc, slow_handle) = PresenceService::with_initial_activity(
            Box::new(slow_mock),
            Some(app_id),
            true,
            Some(make_test_activity("SlowConnect")),
        );
        tokio::spawn(slow_svc.run());

        tokio::time::sleep(Duration::from_millis(30)).await;
        let pause_start = std::time::Instant::now();
        slow_handle.pause().await?;
        let mut slow_rx = slow_handle.subscribe();
        while slow_rx.borrow().state != PresenceState::Disabled {
            slow_rx.changed().await?;
        }
        assert!(pause_start.elapsed() < Duration::from_millis(400));

        let shutdown_start = std::time::Instant::now();
        slow_handle.shutdown().await?;
        assert!(shutdown_start.elapsed() < Duration::from_millis(400));
        Ok(())
    }

    #[tokio::test]
    async fn def07_session_elapsed_and_countdown_timestamps_behave_accurately(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let clock = Arc::new(AtomicU64::new(1_700_001_000));

        let mut initial = make_test_activity("Session 1");
        initial.timestamp_mode = TimestampMode::SessionElapsed;

        let (mut service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(initial),
        );
        service.clock_override_unix = Some(clock.clone());
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.start_timestamp),
            Some(1_700_001_000)
        );

        // Editing details while still connected preserves session_start_unix
        clock.store(1_700_001_050, Ordering::SeqCst);
        let mut edited = make_test_activity("Session 1 Edited");
        edited.timestamp_mode = TimestampMode::SessionElapsed;
        handle.set_activity(edited).await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Session 1 Edited")
        {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.start_timestamp),
            Some(1_700_001_000)
        );

        // Reconnect resets session_start_unix to new publish time
        clock.store(1_700_002_000, Ordering::SeqCst);
        event_tx
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;
        while rx.borrow().is_connected {
            rx.changed().await?;
        }
        handle.retry_now().await?;
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.start_timestamp),
            Some(1_700_002_000)
        );

        // Pause -> Resume resets session_start_unix as well
        clock.store(1_700_003_000, Ordering::SeqCst);
        handle.pause().await?;
        while rx.borrow().state != PresenceState::Disabled {
            rx.changed().await?;
        }
        let mut resumed_session = make_test_activity("Resumed Session");
        resumed_session.timestamp_mode = TimestampMode::SessionElapsed;
        handle
            .resume_with_profile(Some(app_id), Some(resumed_session))
            .await?;
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.start_timestamp),
            Some(1_700_003_000)
        );

        // CountdownUntil: future preserved, expired omitted
        let mut future_cd = make_test_activity("Future Countdown");
        future_cd.timestamp_mode = TimestampMode::CountdownUntil;
        future_cd.end_timestamp = Some(1_700_005_000);
        handle.set_activity(future_cd).await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Future Countdown")
        {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.end_timestamp),
            Some(1_700_005_000)
        );

        clock.store(1_700_006_000, Ordering::SeqCst);
        let mut expired_cd = make_test_activity("Expired Countdown");
        expired_cd.timestamp_mode = TimestampMode::CountdownUntil;
        expired_cd.end_timestamp = Some(1_700_005_000);
        handle.set_activity(expired_cd).await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.details.as_deref())
            != Some("Expired Countdown")
        {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.end_timestamp),
            None
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def09_shutdown_awaits_clear_and_close_completion_and_handles_failures(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        // 1. Delayed clear_activity (150ms) + delayed disconnect (150ms): shutdown future stays pending until BOTH finish
        let mut delayed_mock = MockDiscordAdapter::new();
        let calls = delayed_mock.shared_calls();
        delayed_mock.set_clear_delay(Duration::from_millis(150));
        delayed_mock.set_disconnect_delay(Duration::from_millis(150));

        let app_id = ApplicationId::parse("123456789012345678")?;
        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(delayed_mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Active")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }

        let start = std::time::Instant::now();
        handle.shutdown().await?;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(250),
            "shutdown returned too early ({elapsed:?}) without awaiting clear+close"
        );
        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert!(recorded.contains(&MockAdapterCall::ClearActivity));
        assert!(recorded.contains(&MockAdapterCall::Disconnect));

        // Subsequent command after shutdown must fail cleanly (channel closed, no publish)
        assert!(handle
            .set_activity(make_test_activity("AfterShutdown"))
            .await
            .is_err());

        // 2. Clear failure + disconnect failure still complete shutdown cleanly
        let mut failing_mock = MockDiscordAdapter::new();
        failing_mock.push_clear_result(Err(DiscordError::Io("clear failed".to_owned())));
        failing_mock.push_disconnect_result(Err(DiscordError::Io("close failed".to_owned())));
        let (svc2, handle2) = PresenceService::with_initial_activity(
            Box::new(failing_mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("Active2")),
        );
        tokio::spawn(svc2.run());
        let mut rx2 = handle2.subscribe();
        while rx2.borrow().state != PresenceState::Published {
            rx2.changed().await?;
        }
        handle2.shutdown().await?;
        assert_eq!(rx2.borrow().state, PresenceState::Exiting);

        // 3. Hung clear_activity (> 1000ms) times out at 1s and still calls disconnect()
        let mut hung_clear_mock = MockDiscordAdapter::new();
        let hung_calls = hung_clear_mock.shared_calls();
        hung_clear_mock.set_clear_delay(Duration::from_secs(3));
        let (svc3, handle3) = PresenceService::with_initial_activity(
            Box::new(hung_clear_mock),
            Some(app_id),
            true,
            Some(make_test_activity("HungClear")),
        );
        tokio::spawn(svc3.run());
        let mut rx3 = handle3.subscribe();
        while rx3.borrow().state != PresenceState::Published {
            rx3.changed().await?;
        }
        let t3 = std::time::Instant::now();
        handle3.shutdown().await?;
        let e3 = t3.elapsed();
        assert!(
            e3 >= Duration::from_millis(900) && e3 < Duration::from_millis(2000),
            "expected ~1s clear timeout + disconnect, got {e3:?}"
        );
        assert!(hung_calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .contains(&MockAdapterCall::Disconnect));

        Ok(())
    }

    #[tokio::test]
    async fn def07_countdown_expiry_dedup_republishes_once_and_skips_redundant_updates(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let clock = Arc::new(AtomicU64::new(1_700_001_000));

        let mut countdown_act = make_test_activity("Countdown");
        countdown_act.timestamp_mode = TimestampMode::CountdownUntil;
        countdown_act.end_timestamp = Some(1_700_002_000);

        let (mut service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(countdown_act.clone()),
        );
        service.clock_override_unix = Some(clock.clone());
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::Published {
            rx.changed().await?;
        }
        assert_eq!(
            rx.borrow()
                .active_activity
                .as_ref()
                .and_then(|a| a.end_timestamp),
            Some(1_700_002_000)
        );

        // Advance clock past countdown expiration and re-apply same profile:
        // must NOT falsely deduplicate against the still-active Some(2000) snapshot!
        clock.store(1_700_002_500, Ordering::SeqCst);
        handle
            .apply_profile(Some(app_id.clone()), countdown_act.clone())
            .await?;
        while rx
            .borrow()
            .active_activity
            .as_ref()
            .and_then(|a| a.end_timestamp)
            .is_some()
        {
            rx.changed().await?;
        }

        let set_count_after_expiry = calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .iter()
            .filter(|c| matches!(c, MockAdapterCall::SetActivity { .. }))
            .count();
        assert_eq!(set_count_after_expiry, 2);

        // Re-applying the already-expired profile again MUST deduplicate (None == None)
        handle.apply_profile(Some(app_id), countdown_act).await?;
        tokio::time::sleep(Duration::from_millis(40)).await;
        let set_count_final = calls
            .lock()
            .map_err(|_| "lock poisoned")?
            .iter()
            .filter(|c| matches!(c, MockAdapterCall::SetActivity { .. }))
            .count();
        assert_eq!(set_count_final, 2);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn def05_repeated_rate_limit_never_shortens_active_cooldown(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut mock = MockDiscordAdapter::new();
        // Inject initial SET_ACTIVITY RateLimited(2s), then a shorter RateLimited(1s) via event
        mock.push_set_activity_result(Err(DiscordError::RateLimited {
            retry_after_secs: Some(2),
        }));
        let event_tx = mock.event_sender();
        let app_id = ApplicationId::parse("123456789012345678")?;

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("RateLimitedFirst")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        while rx.borrow().state != PresenceState::WaitingForDiscord
            && rx.borrow().state != PresenceState::Reconnecting
        {
            rx.changed().await?;
        }

        // Send a shorter 1s rate limit event while 2s cooldown is active;
        // must NOT shorten the 2s cooldown so publish stays blocked at 1.2s.
        event_tx
            .send(AdapterEvent::RateLimited {
                retry_after_secs: Some(1),
            })
            .await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_ne!(rx.borrow().state, PresenceState::Published);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap02_pause_surfaces_sanitized_error_and_still_disconnects_on_clear_failure(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        // 1. ClearTimeout during Pause -> Disabled, Disconnect still called, sanitized error set
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(DiscordError::ClearTimeout));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id.clone()),
                true,
                Some(make_test_activity("PublishedBeforeClearTimeout")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published (ClearTimeout)")??;

            handle.pause().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Disabled),
            )
            .await
            .map_err(|_| "timed out waiting for Disabled (ClearTimeout)")??;

            let status = rx.borrow().clone();
            assert!(!status.is_connected);
            assert_eq!(status.active_activity, None);
            let err_msg = status
                .last_error
                .as_deref()
                .ok_or("expected last_error when clear_activity() fails with ClearTimeout")?;
            assert!(!err_msg.contains("Retrying automatically"));
            assert!(err_msg.to_ascii_lowercase().contains("clear"));

            let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
            let clear_idx = recorded
                .iter()
                .position(|c| matches!(c, MockAdapterCall::ClearActivity))
                .ok_or("missing ClearActivity call")?;
            let disc_idx = recorded
                .iter()
                .position(|c| matches!(c, MockAdapterCall::Disconnect))
                .ok_or("missing Disconnect call")?;
            assert_eq!(disc_idx, clear_idx + 1);

            handle.shutdown().await?;
        }

        // 2. RateLimited during Pause -> Disabled, Disconnect still called, rate-limit clear warning
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(DiscordError::RateLimited {
                retry_after_secs: Some(15),
            }));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id.clone()),
                true,
                Some(make_test_activity("PublishedBeforeRateLimitClear")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published (RateLimited)")??;

            handle.pause().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Disabled),
            )
            .await
            .map_err(|_| "timed out waiting for Disabled (RateLimited)")??;

            let status = rx.borrow().clone();
            assert!(!status.is_connected);
            assert_eq!(status.active_activity, None);
            let err_msg = status
                .last_error
                .as_deref()
                .ok_or("expected last_error when clear_activity() fails with RateLimited")?;
            assert!(err_msg.to_ascii_lowercase().contains("rate-limit"));
            assert!(!err_msg.contains("Retrying automatically"));
            assert!(calls
                .lock()
                .map_err(|_| "lock poisoned")?
                .contains(&MockAdapterCall::Disconnect));

            handle.shutdown().await?;
        }

        // 3. RpcRejected & Io during Pause -> sanitized error without peer message or OS paths
        for injected in [
            DiscordError::RpcRejected {
                code: 5000,
                message: "RAW_SECRET_PEER_MSG".to_owned(),
            },
            DiscordError::Io(r"C:\Users\Alice\pipe".to_owned()),
        ] {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(injected));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id.clone()),
                true,
                Some(make_test_activity("PublishedBeforeSanitizedClearErr")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published (sanitized error)")??;

            handle.pause().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Disabled),
            )
            .await
            .map_err(|_| "timed out waiting for Disabled (sanitized error)")??;

            let status = rx.borrow().clone();
            assert!(!status.is_connected);
            assert_eq!(status.active_activity, None);
            let err_msg = status
                .last_error
                .as_deref()
                .ok_or("expected sanitized last_error on clear failure")?;
            assert!(!err_msg.contains("RAW_SECRET_PEER_MSG"));
            assert!(!err_msg.contains("Alice"));
            assert!(!err_msg.contains(r"C:\"));
            assert!(!err_msg.contains("Review your profile"));
            assert!(!err_msg.contains("Retrying automatically"));
            assert!(calls
                .lock()
                .map_err(|_| "lock poisoned")?
                .contains(&MockAdapterCall::Disconnect));

            handle.shutdown().await?;
        }

        // 4. Refinement 2: clear_activity() == Ok(()) + disconnect() == Err(_) MUST NOT surface unconfirmed clear
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Ok(()));
            mock.push_disconnect_result(Err(DiscordError::Io("close failed".to_owned())));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id),
                true,
                Some(make_test_activity("PublishedBeforeDisconnectErr")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published (disconnect error)")??;

            handle.pause().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Disabled),
            )
            .await
            .map_err(|_| "timed out waiting for Disabled (disconnect error)")??;

            let status = rx.borrow().clone();
            assert!(!status.is_connected);
            assert_eq!(status.active_activity, None);
            assert_eq!(
                status.last_error, None,
                "successful clear followed by best-effort disconnect error must not report unconfirmed clear"
            );
            let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
            assert!(recorded.contains(&MockAdapterCall::ClearActivity));
            assert!(recorded.contains(&MockAdapterCall::Disconnect));

            handle.shutdown().await?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn gap02_clear_activity_command_surfaces_errors_and_handles_transport_vs_rpc_rejection(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        // 1. ClearTimeout via ClearActivity command -> disconnects cleanly, sets last_error, does not re-stage cleared activity
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(DiscordError::ClearTimeout));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id.clone()),
                true,
                Some(make_test_activity("ClearCmdTimeout")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published")??;

            handle.clear_activity().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| !s.is_connected && s.last_error.is_some()),
            )
            .await
            .map_err(|_| "timed out waiting for ClearTimeout error surfacing")??;

            let status = rx.borrow().clone();
            assert_eq!(status.active_activity, None);
            assert_eq!(status.pending_activity, None);
            let err_msg = status
                .last_error
                .as_deref()
                .ok_or("expected clear timeout last_error")?;
            assert!(err_msg.to_ascii_lowercase().contains("clear"));
            assert!(calls
                .lock()
                .map_err(|_| "lock poisoned")?
                .contains(&MockAdapterCall::Disconnect));

            handle.shutdown().await?;
        }

        // 2. RateLimited via ClearActivity command -> disconnects, sets last_error, does not re-stage cleared activity
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(DiscordError::RateLimited {
                retry_after_secs: Some(2),
            }));
            let calls = mock.shared_calls();

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id.clone()),
                true,
                Some(make_test_activity("ClearCmdRateLimit")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published")??;

            handle.clear_activity().await?;
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| !s.is_connected && s.last_error.is_some()),
            )
            .await
            .map_err(|_| "timed out waiting for RateLimited clear error surfacing")??;

            let status = rx.borrow().clone();
            assert_eq!(status.active_activity, None);
            assert_eq!(status.pending_activity, None);
            assert!(calls
                .lock()
                .map_err(|_| "lock poisoned")?
                .contains(&MockAdapterCall::Disconnect));

            handle.shutdown().await?;
        }

        // 3. Non-retryable RpcRejected while pipe stays connected -> sets sanitized last_error and does NOT falsely clear active_activity
        {
            let mut mock = MockDiscordAdapter::new();
            mock.push_clear_result(Err(DiscordError::RpcRejected {
                code: 5000,
                message: "RAW_REJECT_MESSAGE".to_owned(),
            }));

            let (service, handle) = PresenceService::with_initial_activity(
                Box::new(mock),
                Some(app_id),
                true,
                Some(make_test_activity("ClearCmdRpcRejected")),
            );
            tokio::spawn(service.run());

            let mut rx = handle.subscribe();
            tokio::time::timeout(
                wait_limit,
                rx.wait_for(|s| s.state == PresenceState::Published),
            )
            .await
            .map_err(|_| "timed out waiting for Published")??;

            handle.clear_activity().await?;
            tokio::time::timeout(wait_limit, rx.wait_for(|s| s.last_error.is_some()))
                .await
                .map_err(|_| "timed out waiting for RpcRejected last_error")??;

            let status = rx.borrow().clone();
            assert!(status.is_connected);
            assert!(
                status.active_activity.is_some(),
                "must not claim active_activity was cleared when Discord rejected the RPC clear"
            );
            let err_msg = status
                .last_error
                .as_deref()
                .ok_or("expected RpcRejected last_error")?;
            assert!(!err_msg.contains("RAW_REJECT_MESSAGE"));
            assert!(!err_msg.contains("Review your profile"));

            handle.shutdown().await?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn gap05_pause_invokes_clear_activity_before_disconnect_and_is_idempotent(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("PublishedOrderCheck")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled on first pause")??;

        handle.pause().await?;
        tokio::time::timeout(wait_limit, rx.changed())
            .await
            .map_err(|_| "timed out waiting for broadcast on second pause")??;

        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert!(!status.is_connected);
        assert_eq!(status.active_activity, None);
        assert_eq!(status.pending_activity, None);
        assert_eq!(status.last_error, None);

        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert_eq!(recorded.len(), 4);
        assert!(matches!(recorded[0], MockAdapterCall::Connect { .. }));
        assert!(matches!(recorded[1], MockAdapterCall::SetActivity { .. }));
        assert_eq!(recorded[2], MockAdapterCall::ClearActivity);
        assert_eq!(recorded[3], MockAdapterCall::Disconnect);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_pause_during_reconnecting_cancels_backoff_and_stays_disabled(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("PublishedBeforeDisconnect")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        event_tx
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Reconnecting),
        )
        .await
        .map_err(|_| "timed out waiting for Reconnecting")??;

        let calls_before_pause = calls.lock().map_err(|_| "lock poisoned")?.len();
        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled")??;

        // Backoff attempt 1 is 1s +/- 20% (<= 1200ms); verify zero status changes or adapter calls over 1400ms
        assert!(
            tokio::time::timeout(Duration::from_millis(1400), rx.changed())
                .await
                .is_err(),
            "unexpected reconnect/publish event after Pause during Reconnecting"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert!(!status.is_connected);
        assert_eq!(status.active_activity, None);
        assert_eq!(status.last_error, None);

        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert_eq!(
            recorded.len(),
            calls_before_pause,
            "expected zero Connect/SetActivity/ClearActivity calls after Pause while already disconnected"
        );
        assert!(!recorded[calls_before_pause..].iter().any(|c| matches!(
            c,
            MockAdapterCall::Connect { .. }
                | MockAdapterCall::SetActivity { .. }
                | MockAdapterCall::ClearActivity
        )));

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_pause_during_client_rate_limit_cooldown_clears_immediately(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (mut service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("RateLimitSlot1")),
        );
        service.rate_limiter = RollingRateLimiter::new(1, Duration::from_secs(5));
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for initial Published")??;

        // Queue second update; wait for pending snapshot, then wait for debounce expiry to arm 5s rate_limit_timer
        handle
            .set_activity(make_test_activity("RateLimitBlocked2"))
            .await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| {
                s.pending_activity
                    .as_ref()
                    .and_then(|a| a.details.as_deref())
                    == Some("RateLimitBlocked2")
            }),
        )
        .await
        .map_err(|_| "timed out waiting for pending RateLimitBlocked2")??;

        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .map_err(|_| "timed out waiting for rate_limit_timer arming broadcast")??;

        let pause_start = std::time::Instant::now();
        handle.pause().await?;
        tokio::time::timeout(
            Duration::from_millis(500),
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "Pause was blocked or delayed by client rate-limit cooldown")??;
        assert!(pause_start.elapsed() < Duration::from_millis(500));

        assert!(
            tokio::time::timeout(Duration::from_millis(700), rx.changed())
                .await
                .is_err(),
            "unexpected publish/status update while paused during client cooldown"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert!(!status.is_connected);
        assert_eq!(status.active_activity, None);
        assert_eq!(status.pending_activity, None);
        assert_eq!(status.last_error, None);

        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert!(recorded.contains(&MockAdapterCall::ClearActivity));
        assert!(recorded.contains(&MockAdapterCall::Disconnect));
        assert!(!recorded.iter().any(|c| matches!(
            c,
            MockAdapterCall::SetActivity { activity }
                if activity.details.as_deref() == Some("RateLimitBlocked2")
        )));

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_pause_during_discord_rate_limit_until_cancels_reconnect(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("BeforeDiscord4002")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        event_tx
            .send(AdapterEvent::RateLimited {
                retry_after_secs: Some(1),
            })
            .await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Reconnecting),
        )
        .await
        .map_err(|_| "timed out waiting for Reconnecting on 4002")??;

        let calls_before_pause = calls.lock().map_err(|_| "lock poisoned")?.len();
        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled")??;

        assert!(
            tokio::time::timeout(Duration::from_millis(1300), rx.changed())
                .await
                .is_err(),
            "unexpected reconnect after 1s Discord rate-limit cooldown while Paused"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert_eq!(status.pending_activity, None);
        assert_eq!(status.last_error, None);
        assert_eq!(
            calls.lock().map_err(|_| "lock poisoned")?.len(),
            calls_before_pause
        );

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_resume_after_clean_and_failed_clear_pause_restores_published_and_clears_error(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mut mock = MockDiscordAdapter::new();
        mock.push_clear_result(Err(DiscordError::ClearTimeout));
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id.clone()),
            true,
            Some(make_test_activity("InitialBeforeFailedPause")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for initial Published")??;

        // 1. Pause with failed clear -> Disabled + last_error present
        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled && s.last_error.is_some()),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled with last_error")??;

        // 2. Resume after failed-clear Pause -> restores Published and clears last_error
        handle
            .resume_with_profile(
                Some(app_id),
                Some(make_test_activity("ResumedAfterFailedClear")),
            )
            .await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| {
                s.state == PresenceState::Published
                    && s.active_activity
                        .as_ref()
                        .and_then(|a| a.details.as_deref())
                        == Some("ResumedAfterFailedClear")
            }),
        )
        .await
        .map_err(|_| "timed out waiting for Published after Resume")??;
        assert_eq!(rx.borrow().last_error, None);

        // 3. Clean Pause -> Disabled with last_error == None, then Resume again
        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled on clean Pause")??;
        assert_eq!(rx.borrow().last_error, None);

        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert_eq!(recorded.len(), 8);
        assert!(matches!(recorded[0], MockAdapterCall::Connect { .. }));
        assert!(matches!(recorded[1], MockAdapterCall::SetActivity { .. }));
        assert_eq!(recorded[2], MockAdapterCall::ClearActivity);
        assert_eq!(recorded[3], MockAdapterCall::Disconnect);
        assert!(matches!(recorded[4], MockAdapterCall::Connect { .. }));
        assert!(matches!(recorded[5], MockAdapterCall::SetActivity { .. }));
        assert_eq!(recorded[6], MockAdapterCall::ClearActivity);
        assert_eq!(recorded[7], MockAdapterCall::Disconnect);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_disconnect_error_during_pause_clear_still_tears_down_cleanly(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mut mock = MockDiscordAdapter::new();
        mock.set_clear_delay(Duration::from_millis(100));
        mock.push_clear_result(Err(DiscordError::Disconnected));
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("PublishedBeforeClearDisconnect")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        handle.pause().await?;
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled")??;

        assert!(
            tokio::time::timeout(Duration::from_millis(1300), rx.changed())
                .await
                .is_err(),
            "unexpected reconnect after Disconnected during Pause clear"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert!(!status.is_connected);
        assert_eq!(status.active_activity, None);
        assert_eq!(
            status.last_error.as_deref(),
            Some(DiscordError::Disconnected.clear_failure_message())
        );

        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        let clear_idx = recorded
            .iter()
            .position(|c| matches!(c, MockAdapterCall::ClearActivity))
            .ok_or("missing ClearActivity")?;
        let disc_idx = recorded
            .iter()
            .position(|c| matches!(c, MockAdapterCall::Disconnect))
            .ok_or("missing Disconnect")?;
        assert_eq!(disc_idx, clear_idx + 1);

        handle.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn gap05_concurrent_adapter_disconnect_and_pause_settles_to_disabled(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::discord::MockAdapterCall;

        let mock = MockDiscordAdapter::new();
        let event_tx = mock.event_sender();
        let calls = mock.shared_calls();
        let app_id = ApplicationId::parse("123456789012345678")?;
        let wait_limit = Duration::from_secs(3);

        let (service, handle) = PresenceService::with_initial_activity(
            Box::new(mock),
            Some(app_id),
            true,
            Some(make_test_activity("PublishedBeforeRace")),
        );
        tokio::spawn(service.run());

        let mut rx = handle.subscribe();
        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Published),
        )
        .await
        .map_err(|_| "timed out waiting for Published")??;

        event_tx
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;
        handle.pause().await?;

        tokio::time::timeout(
            wait_limit,
            rx.wait_for(|s| s.state == PresenceState::Disabled),
        )
        .await
        .map_err(|_| "timed out waiting for Disabled after concurrent disconnect + pause")??;

        let calls_after_disabled = calls.lock().map_err(|_| "lock poisoned")?.len();
        assert!(
            tokio::time::timeout(Duration::from_millis(1300), rx.changed())
                .await
                .is_err(),
            "unexpected reconnect after concurrent disconnect + Pause"
        );
        let status = rx.borrow().clone();
        assert_eq!(status.state, PresenceState::Disabled);
        assert!(!status.is_connected);
        let recorded = calls.lock().map_err(|_| "lock poisoned")?.clone();
        assert_eq!(recorded.len(), calls_after_disabled);
        assert_eq!(
            recorded
                .iter()
                .filter(|c| matches!(c, MockAdapterCall::Connect { .. }))
                .count(),
            1
        );

        handle.shutdown().await?;
        Ok(())
    }
}
