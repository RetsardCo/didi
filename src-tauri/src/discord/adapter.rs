use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::discord::DiscordError;
use crate::presence::{ApplicationId, PresenceActivity};

/// Non-PII metadata returned after a successful Discord RPC handshake (D-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub protocol_version: u32,
    pub pipe_slot: u8,
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterEvent {
    Disconnected { reason: DiscordError },
    RpcError { code: i32, message: String },
    RateLimited { retry_after_secs: Option<u64> },
}

/// Swappable Discord local presence transport contract (ADR D-012),
/// implemented by `LiveDiscordIpcAdapter` and `MockDiscordAdapter`.
#[async_trait]
pub trait DiscordPresenceAdapter: Send {
    async fn connect(
        &mut self,
        application_id: &ApplicationId,
    ) -> Result<SessionInfo, DiscordError>;
    async fn set_activity(&mut self, activity: &PresenceActivity) -> Result<(), DiscordError>;
    async fn clear_activity(&mut self) -> Result<(), DiscordError>;
    async fn next_event(&mut self) -> AdapterEvent;
    async fn disconnect(&mut self) -> Result<(), DiscordError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockAdapterCall {
    Connect { masked_app_id: String },
    SetActivity { activity: Box<PresenceActivity> },
    ClearActivity,
    Disconnect,
}

/// Deterministic in-memory test double for Phase 2 scaffold and future state-machine tests.
pub struct MockDiscordAdapter {
    connected: bool,
    calls: Vec<MockAdapterCall>,
    shared_calls: Arc<Mutex<Vec<MockAdapterCall>>>,
    connect_results: VecDeque<Result<SessionInfo, DiscordError>>,
    set_activity_results: VecDeque<Result<(), DiscordError>>,
    clear_results: VecDeque<Result<(), DiscordError>>,
    disconnect_results: VecDeque<Result<(), DiscordError>>,
    connect_delay: Duration,
    clear_delay: Duration,
    disconnect_delay: Duration,
    event_rx: mpsc::Receiver<AdapterEvent>,
    event_tx: mpsc::Sender<AdapterEvent>,
}

impl Default for MockDiscordAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDiscordAdapter {
    #[must_use]
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(16);
        Self {
            connected: false,
            calls: Vec::new(),
            shared_calls: Arc::new(Mutex::new(Vec::new())),
            connect_results: VecDeque::new(),
            set_activity_results: VecDeque::new(),
            clear_results: VecDeque::new(),
            disconnect_results: VecDeque::new(),
            connect_delay: Duration::ZERO,
            clear_delay: Duration::ZERO,
            disconnect_delay: Duration::ZERO,
            event_rx,
            event_tx,
        }
    }

    pub fn set_connect_delay(&mut self, delay: Duration) {
        self.connect_delay = delay;
    }

    pub fn set_clear_delay(&mut self, delay: Duration) {
        self.clear_delay = delay;
    }

    pub fn set_disconnect_delay(&mut self, delay: Duration) {
        self.disconnect_delay = delay;
    }

    pub fn push_disconnect_result(&mut self, result: Result<(), DiscordError>) {
        self.disconnect_results.push_back(result);
    }

    #[must_use]
    pub fn event_sender(&self) -> mpsc::Sender<AdapterEvent> {
        self.event_tx.clone()
    }

    pub fn push_connect_result(&mut self, result: Result<SessionInfo, DiscordError>) {
        self.connect_results.push_back(result);
    }

    pub fn push_set_activity_result(&mut self, result: Result<(), DiscordError>) {
        self.set_activity_results.push_back(result);
    }

    pub fn push_clear_result(&mut self, result: Result<(), DiscordError>) {
        self.clear_results.push_back(result);
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    #[must_use]
    pub fn calls(&self) -> &[MockAdapterCall] {
        &self.calls
    }

    #[must_use]
    pub fn shared_calls(&self) -> Arc<Mutex<Vec<MockAdapterCall>>> {
        self.shared_calls.clone()
    }

    fn record_call(&mut self, call: MockAdapterCall) {
        self.calls.push(call.clone());
        if let Ok(mut guard) = self.shared_calls.lock() {
            guard.push(call);
        }
    }
}

#[async_trait]
impl DiscordPresenceAdapter for MockDiscordAdapter {
    async fn connect(
        &mut self,
        application_id: &ApplicationId,
    ) -> Result<SessionInfo, DiscordError> {
        self.record_call(MockAdapterCall::Connect {
            masked_app_id: application_id.masked(),
        });
        if !self.connect_delay.is_zero() {
            tokio::time::sleep(self.connect_delay).await;
        }
        let result = self.connect_results.pop_front().unwrap_or_else(|| {
            Ok(SessionInfo {
                protocol_version: 1,
                pipe_slot: 0,
                environment: Some("mock".to_owned()),
            })
        });
        self.connected = result.is_ok();
        result
    }

    async fn set_activity(&mut self, activity: &PresenceActivity) -> Result<(), DiscordError> {
        self.record_call(MockAdapterCall::SetActivity {
            activity: Box::new(activity.clone()),
        });
        if !self.connected {
            return Err(DiscordError::NotConnected);
        }
        self.set_activity_results.pop_front().unwrap_or(Ok(()))
    }

    async fn clear_activity(&mut self) -> Result<(), DiscordError> {
        self.record_call(MockAdapterCall::ClearActivity);
        if !self.clear_delay.is_zero() {
            tokio::time::sleep(self.clear_delay).await;
        }
        if !self.connected {
            return Err(DiscordError::NotConnected);
        }
        self.clear_results.pop_front().unwrap_or(Ok(()))
    }

    async fn next_event(&mut self) -> AdapterEvent {
        self.event_rx
            .recv()
            .await
            .unwrap_or(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
    }

    async fn disconnect(&mut self) -> Result<(), DiscordError> {
        self.record_call(MockAdapterCall::Disconnect);
        if !self.disconnect_delay.is_zero() {
            tokio::time::sleep(self.disconnect_delay).await;
        }
        self.connected = false;
        self.disconnect_results.pop_front().unwrap_or(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{ActivityType, ArtworkRef, StatusDisplayType, TimestampMode};

    fn sample_activity() -> PresenceActivity {
        PresenceActivity {
            activity_type: ActivityType::Playing,
            status_display_type: Some(StatusDisplayType::Name),
            name_override: None,
            details: Some("Scaffold Verification".to_owned()),
            details_url: None,
            state: Some("Idle".to_owned()),
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
    async fn mock_adapter_enforces_connection_lifecycle_and_masks_app_id(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = MockDiscordAdapter::new();
        let app_id = ApplicationId::parse("123456789012345678")?;

        // Publishing before connecting must fail with NotConnected.
        assert_eq!(
            adapter.set_activity(&sample_activity()).await,
            Err(DiscordError::NotConnected)
        );

        let session = adapter.connect(&app_id).await?;
        assert_eq!(session.protocol_version, 1);
        assert!(adapter.is_connected());

        assert_eq!(adapter.set_activity(&sample_activity()).await, Ok(()));
        assert_eq!(adapter.clear_activity().await, Ok(()));
        assert_eq!(adapter.disconnect().await, Ok(()));
        assert!(!adapter.is_connected());

        assert_eq!(
            adapter.calls().first(),
            Some(&MockAdapterCall::SetActivity {
                activity: Box::new(sample_activity())
            })
        );
        assert_eq!(
            adapter.calls().get(1),
            Some(&MockAdapterCall::Connect {
                masked_app_id: "****5678".to_owned()
            })
        );
        Ok(())
    }

    #[tokio::test]
    async fn mock_adapter_propagates_injected_errors_and_disconnect_events(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut adapter = MockDiscordAdapter::new();
        let sender = adapter.event_sender();
        let app_id = ApplicationId::parse("987654321098765432")?;

        adapter.push_connect_result(Err(DiscordError::InvalidClientId));
        assert_eq!(
            adapter.connect(&app_id).await,
            Err(DiscordError::InvalidClientId)
        );
        assert!(!adapter.is_connected());

        sender
            .send(AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected,
            })
            .await?;

        let evt = adapter.next_event().await;
        assert_eq!(
            evt,
            AdapterEvent::Disconnected {
                reason: DiscordError::Disconnected
            }
        );
        Ok(())
    }
}
