use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

#[cfg(windows)]
use crate::discord::windows_pipe::connect_discord_pipe;
#[cfg(windows)]
use tokio::io::split;

use crate::discord::codec::{read_frame, write_frame, Opcode};
use crate::discord::payload::{
    HandshakePayload, ReadyPayload, RpcErrorData, RpcResponse, SetActivityCommand, WireActivity,
};
use crate::discord::{AdapterEvent, DiscordError, DiscordPresenceAdapter, SessionInfo};
use crate::presence::{ApplicationId, PresenceActivity};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const SET_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(5);
const CLEAR_TIMEOUT: Duration = Duration::from_secs(2);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const PONG_TIMEOUT: Duration = Duration::from_secs(1);

type PendingNonces = Arc<Mutex<HashMap<String, oneshot::Sender<RpcResponse>>>>;
type WriterMutex = Arc<Mutex<Box<dyn AsyncWrite + Send + Unpin>>>;

/// Concrete Discord RPC-over-IPC adapter communicating directly with a running Discord client
/// through Windows named pipes (`\\.\pipe\discord-ipc-0..9`) per ADR D-012.
pub struct LiveDiscordIpcAdapter {
    writer: Option<WriterMutex>,
    pending_nonces: PendingNonces,
    reader_handle: Option<JoinHandle<()>>,
    session_info: Option<SessionInfo>,
    event_rx: mpsc::Receiver<AdapterEvent>,
    event_tx: mpsc::Sender<AdapterEvent>,
}

impl Default for LiveDiscordIpcAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveDiscordIpcAdapter {
    #[must_use]
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel(32);
        Self {
            writer: None,
            pending_nonces: Arc::new(Mutex::new(HashMap::new())),
            reader_handle: None,
            session_info: None,
            event_rx,
            event_tx,
        }
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.session_info.is_some()
    }

    #[must_use]
    pub fn session_info(&self) -> Option<&SessionInfo> {
        self.session_info.as_ref()
    }

    #[must_use]
    pub fn event_sender(&self) -> mpsc::Sender<AdapterEvent> {
        self.event_tx.clone()
    }

    /// Connects using an arbitrary async I/O pair. Enables deterministic wire-protocol testing
    /// with `tokio::io::duplex` alongside live Windows named-pipe execution.
    pub async fn connect_with_io<R, W>(
        &mut self,
        mut reader: R,
        mut writer: W,
        slot: u8,
        application_id: &ApplicationId,
    ) -> Result<SessionInfo, DiscordError>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let _ = self.disconnect().await;

        // Drain any stale events from a previous connection
        while self.event_rx.try_recv().is_ok() {}

        // 1. Transmit Opcode 0 (HANDSHAKE)
        let handshake_bytes = HandshakePayload::new(application_id).to_json_bytes()?;
        tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            write_frame(&mut writer, Opcode::Handshake, &handshake_bytes),
        )
        .await
        .map_err(|_| DiscordError::HandshakeTimeout)??;

        // 2. Await Opcode 1 (READY) dispatch within 5s
        let (opcode, ready_bytes) =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
                .await
                .map_err(|_| DiscordError::HandshakeTimeout)??;

        if opcode == Opcode::Close {
            if let Ok(err) = serde_json::from_slice::<RpcErrorData>(&ready_bytes) {
                return Err(err.into_discord_error());
            }
            return Err(DiscordError::Disconnected);
        }

        if opcode != Opcode::Frame {
            return Err(DiscordError::ProtocolViolation);
        }

        // 3. Validate READY response without retaining any PII (ADR D-022)
        let ready: ReadyPayload = serde_json::from_slice(&ready_bytes).map_err(|_| {
            if let Ok(err_resp) = serde_json::from_slice::<RpcResponse>(&ready_bytes) {
                if let Err(mapped_err) = err_resp.check_ack("") {
                    return mapped_err;
                }
            }
            DiscordError::ProtocolViolation
        })?;

        if ready.cmd != "DISPATCH" || ready.evt.as_deref() != Some("READY") {
            return Err(DiscordError::ProtocolMismatch { version: 0 });
        }

        let version = ready
            .data
            .as_ref()
            .map(|d| d.v)
            .ok_or(DiscordError::ProtocolViolation)?;
        if version != 1 {
            return Err(DiscordError::ProtocolMismatch { version });
        }

        let session = SessionInfo {
            protocol_version: version,
            pipe_slot: slot,
            environment: None,
        };

        // 4. Setup writer mutex, pending nonce registry, and spawn async background reader task
        let boxed_writer: Box<dyn AsyncWrite + Send + Unpin> = Box::new(writer);
        let writer_mutex = Arc::new(Mutex::new(boxed_writer));
        self.writer = Some(writer_mutex.clone());
        self.session_info = Some(session.clone());

        let pending_nonces = self.pending_nonces.clone();
        let event_tx = self.event_tx.clone();
        let writer_clone = writer_mutex.clone();

        let reader_handle = tokio::spawn(async move {
            run_reader_loop(Box::new(reader), writer_clone, pending_nonces, event_tx).await;
        });
        self.reader_handle = Some(reader_handle);

        Ok(session)
    }

    async fn send_activity_command(
        &mut self,
        payload: &[u8],
        nonce: String,
        clear_deadline: Option<tokio::time::Instant>,
    ) -> Result<(), DiscordError> {
        let writer_mutex = match &self.writer {
            Some(w) => w.clone(),
            None => return Err(DiscordError::NotConnected),
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending_nonces.lock().await;
            map.insert(nonce.clone(), tx);
        }

        let write_fut = async {
            let mut writer = writer_mutex.lock().await;
            write_frame(&mut *writer, Opcode::Frame, payload).await
        };
        let write_res = match clear_deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, write_fut).await,
            None => tokio::time::timeout(SET_ACTIVITY_TIMEOUT, write_fut).await,
        };

        match write_res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let mut map = self.pending_nonces.lock().await;
                map.remove(&nonce);
                return Err(e);
            }
            Err(_) => {
                let mut map = self.pending_nonces.lock().await;
                map.remove(&nonce);
                return Err(if clear_deadline.is_some() {
                    DiscordError::ClearTimeout
                } else {
                    DiscordError::Io("RPC SET_ACTIVITY write timeout".to_owned())
                });
            }
        }

        let read_res = match clear_deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, rx).await,
            None => tokio::time::timeout(SET_ACTIVITY_TIMEOUT, rx).await,
        };

        let response = match read_res {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => return Err(DiscordError::Disconnected),
            Err(_) => {
                let mut map = self.pending_nonces.lock().await;
                map.remove(&nonce);
                return Err(if clear_deadline.is_some() {
                    DiscordError::ClearTimeout
                } else {
                    DiscordError::Io("RPC SET_ACTIVITY timeout".to_owned())
                });
            }
        };

        response.check_ack(&nonce)
    }
}

#[async_trait]
impl DiscordPresenceAdapter for LiveDiscordIpcAdapter {
    #[cfg(windows)]
    async fn connect(
        &mut self,
        application_id: &ApplicationId,
    ) -> Result<SessionInfo, DiscordError> {
        let (pipe, slot) = connect_discord_pipe().await?;
        let (reader, writer) = split(pipe);
        self.connect_with_io(reader, writer, slot, application_id)
            .await
    }

    #[cfg(not(windows))]
    async fn connect(
        &mut self,
        _application_id: &ApplicationId,
    ) -> Result<SessionInfo, DiscordError> {
        Err(DiscordError::DiscordNotRunning)
    }

    async fn set_activity(&mut self, activity: &PresenceActivity) -> Result<(), DiscordError> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let wire = WireActivity::from_domain(activity);
        let cmd = SetActivityCommand::set(std::process::id(), wire, &nonce);
        let payload = cmd.to_json_bytes()?;
        self.send_activity_command(&payload, nonce, None).await
    }

    async fn clear_activity(&mut self) -> Result<(), DiscordError> {
        let deadline = tokio::time::Instant::now() + CLEAR_TIMEOUT;
        let nonce = uuid::Uuid::new_v4().to_string();
        let cmd = SetActivityCommand::clear(std::process::id(), &nonce);
        let payload = cmd.to_json_bytes()?;
        self.send_activity_command(&payload, nonce, Some(deadline))
            .await
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
        if let Some(writer_mutex) = self.writer.take() {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
                let mut writer = writer_mutex.lock().await;
                write_frame(&mut *writer, Opcode::Close, b"{}").await
            })
            .await;
        }

        if let Some(handle) = self.reader_handle.take() {
            handle.abort();
        }

        self.session_info = None;
        let mut map = self.pending_nonces.lock().await;
        map.clear();

        Ok(())
    }
}

async fn run_reader_loop(
    mut reader: Box<dyn AsyncRead + Send + Unpin>,
    writer_mutex: WriterMutex,
    pending_nonces: PendingNonces,
    event_tx: mpsc::Sender<AdapterEvent>,
) {
    loop {
        match read_frame(&mut reader).await {
            Ok((Opcode::Ping, payload)) => {
                let _ = tokio::time::timeout(PONG_TIMEOUT, async {
                    let mut writer = writer_mutex.lock().await;
                    write_frame(&mut *writer, Opcode::Pong, &payload).await
                })
                .await;
            }
            Ok((Opcode::Close, payload)) => {
                let reason = serde_json::from_slice::<RpcErrorData>(&payload)
                    .map_or(DiscordError::Disconnected, RpcErrorData::into_discord_error);
                let _ = event_tx.send(AdapterEvent::Disconnected { reason }).await;
                break;
            }
            Ok((Opcode::Frame, payload)) => {
                if let Ok(resp) = serde_json::from_slice::<RpcResponse>(&payload) {
                    if let Some(nonce) = &resp.nonce {
                        let mut map = pending_nonces.lock().await;
                        if let Some(sender) = map.remove(nonce) {
                            let _ = sender.send(resp);
                            continue;
                        }
                    }

                    if let Some(evt) = &resp.evt {
                        if evt == "ERROR" {
                            let (code, msg) = if let Some(data) = &resp.data {
                                if let Ok(err) =
                                    serde_json::from_value::<RpcErrorData>(data.clone())
                                {
                                    (err.code, err.message)
                                } else {
                                    (-1, "Unsolicited RPC error".to_owned())
                                }
                            } else {
                                (-1, "Unsolicited RPC error".to_owned())
                            };
                            let _ = event_tx
                                .send(AdapterEvent::RpcError { code, message: msg })
                                .await;
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                let _ = event_tx
                    .send(AdapterEvent::Disconnected { reason: e })
                    .await;
                break;
            }
        }
    }

    // Always clear pending nonces when reader loop exits so awaiting callers
    // receive an immediate Disconnected error instead of hanging until timeout.
    let mut map = pending_nonces.lock().await;
    map.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presence::{ActivityType, ArtworkRef, StatusDisplayType, TimestampMode};
    use tokio::io::duplex;

    fn sample_activity() -> PresenceActivity {
        PresenceActivity {
            activity_type: ActivityType::Playing,
            status_display_type: Some(StatusDisplayType::Name),
            name_override: None,
            details: Some("Automated Wire Test".to_owned()),
            details_url: None,
            state: Some("Mock Server".to_owned()),
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
    async fn live_adapter_full_mocked_lifecycle_over_duplex() -> Result<(), DiscordError> {
        let (client_io, server_io) = duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (mut server_read, mut server_write) = tokio::io::split(server_io);

        let app_id = ApplicationId::parse("123456789012345678")
            .map_err(|_| DiscordError::InvalidClientId)?;
        let mut adapter = LiveDiscordIpcAdapter::new();

        // Spawn mock Discord server task
        let server_task = tokio::spawn(async move {
            // 1. Receive Handshake
            let (op, hs_bytes) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Handshake);
            assert!(String::from_utf8_lossy(&hs_bytes).contains("123456789012345678"));

            // 2. Send READY frame
            let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
            write_frame(&mut server_write, Opcode::Frame, ready_json).await?;

            // 3. Receive SET_ACTIVITY
            let (op, sa_bytes) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Frame);
            let sa_obj: serde_json::Value =
                serde_json::from_slice(&sa_bytes).map_err(|e| DiscordError::Io(e.to_string()))?;
            let nonce = sa_obj["nonce"].as_str().unwrap_or("").to_owned();

            // Send ACK for SET_ACTIVITY
            let ack_json = format!(r#"{{"cmd":"SET_ACTIVITY","data":{{}},"nonce":"{nonce}"}}"#);
            write_frame(&mut server_write, Opcode::Frame, ack_json.as_bytes()).await?;

            // 4. Send PING to client
            let ping_payload = b"ping_test_payload";
            write_frame(&mut server_write, Opcode::Ping, ping_payload).await?;

            // 5. Receive PONG reply from client
            let (pong_op, pong_payload) = read_frame(&mut server_read).await?;
            assert_eq!(pong_op, Opcode::Pong);
            assert_eq!(pong_payload, ping_payload);

            // 6. Receive CLEAR_ACTIVITY
            let (op, clear_bytes) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Frame);
            let clear_obj: serde_json::Value = serde_json::from_slice(&clear_bytes)
                .map_err(|e| DiscordError::Io(e.to_string()))?;
            assert_eq!(clear_obj["cmd"].as_str(), Some("SET_ACTIVITY"));
            assert!(clear_obj["args"]["activity"].is_null());
            let clear_nonce = clear_obj["nonce"].as_str().unwrap_or("").to_owned();

            // Send ACK for CLEAR_ACTIVITY matching live Discord Desktop response shape
            let clear_ack = format!(
                r#"{{"cmd":"SET_ACTIVITY","data":null,"evt":null,"nonce":"{clear_nonce}"}}"#
            );
            write_frame(&mut server_write, Opcode::Frame, clear_ack.as_bytes()).await?;

            // 7. Receive CLOSE on disconnect
            let (op, _) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Close);

            Ok::<(), DiscordError>(())
        });

        // Client side executes adapter methods
        let session = adapter
            .connect_with_io(client_read, client_write, 0, &app_id)
            .await?;
        assert_eq!(session.protocol_version, 1);
        assert_eq!(session.pipe_slot, 0);
        assert!(adapter.is_connected());

        adapter.set_activity(&sample_activity()).await?;
        adapter.clear_activity().await?;
        adapter.disconnect().await?;
        assert!(!adapter.is_connected());

        server_task
            .await
            .map_err(|e| DiscordError::Io(e.to_string()))??;
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_while_waiting_for_ack_fails_immediately_without_timeout(
    ) -> Result<(), DiscordError> {
        let (client_io, server_io) = duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (mut server_read, mut server_write) = tokio::io::split(server_io);

        let app_id = ApplicationId::parse("123456789012345678")
            .map_err(|_| DiscordError::InvalidClientId)?;
        let mut adapter = LiveDiscordIpcAdapter::new();

        let server_task = tokio::spawn(async move {
            let (op, _) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Handshake);

            let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
            write_frame(&mut server_write, Opcode::Frame, ready_json).await?;

            // Read the SET_ACTIVITY frame, then abruptly drop the server connection (EOF)
            let (op, _) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Frame);
            drop(server_read);
            drop(server_write);

            Ok::<(), DiscordError>(())
        });

        adapter
            .connect_with_io(client_read, client_write, 0, &app_id)
            .await?;

        let start = std::time::Instant::now();
        let res = adapter.set_activity(&sample_activity()).await;
        let elapsed = start.elapsed();

        // Must fail with Disconnected immediately, definitely well under 5 seconds (e.g. < 500ms)
        assert_eq!(res, Err(DiscordError::Disconnected));
        assert!(
            elapsed < Duration::from_millis(500),
            "Expected immediate disconnect failure, but took {elapsed:?}"
        );

        server_task
            .await
            .map_err(|e| DiscordError::Io(e.to_string()))??;
        Ok(())
    }

    #[tokio::test]
    async fn unsolicited_rpc_error_and_close_frame_dispatch_reasons() -> Result<(), DiscordError> {
        let (client_io, server_io) = duplex(4096);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (mut server_read, mut server_write) = tokio::io::split(server_io);

        let app_id = ApplicationId::parse("123456789012345678")
            .map_err(|_| DiscordError::InvalidClientId)?;
        let mut adapter = LiveDiscordIpcAdapter::new();

        let server_task = tokio::spawn(async move {
            let (op, _) = read_frame(&mut server_read).await?;
            assert_eq!(op, Opcode::Handshake);

            let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
            write_frame(&mut server_write, Opcode::Frame, ready_json).await?;

            // Send unsolicited RPC error
            let err_json = br#"{"cmd":"SET_ACTIVITY","evt":"ERROR","data":{"code":4002,"message":"Rate limit reached"}}"#;
            write_frame(&mut server_write, Opcode::Frame, err_json).await?;

            // Send Opcode::Close with code 4000
            let close_json = br#"{"code":4000,"message":"Invalid Client ID"}"#;
            write_frame(&mut server_write, Opcode::Close, close_json).await?;

            Ok::<(), DiscordError>(())
        });

        adapter
            .connect_with_io(client_read, client_write, 0, &app_id)
            .await?;

        // 1. Next event should be the unsolicited RpcError with actual code & message
        let ev1 = adapter.next_event().await;
        assert_eq!(
            ev1,
            AdapterEvent::RpcError {
                code: 4002,
                message: "Rate limit reached".to_owned()
            }
        );

        // 2. Next event should be Disconnected with InvalidClientId from the Close frame
        let ev2 = adapter.next_event().await;
        assert_eq!(
            ev2,
            AdapterEvent::Disconnected {
                reason: DiscordError::InvalidClientId
            }
        );

        server_task
            .await
            .map_err(|e| DiscordError::Io(e.to_string()))??;
        Ok(())
    }

    #[tokio::test]
    async fn reconnect_drains_stale_events() -> Result<(), DiscordError> {
        let (client_io1, server_io1) = duplex(4096);
        let (client_read1, client_write1) = tokio::io::split(client_io1);
        let (mut server_read1, mut server_write1) = tokio::io::split(server_io1);

        let app_id = ApplicationId::parse("123456789012345678")
            .map_err(|_| DiscordError::InvalidClientId)?;
        let mut adapter = LiveDiscordIpcAdapter::new();

        // Session 1: connects then server closes
        let hs1 = tokio::spawn(async move {
            let _ = read_frame(&mut server_read1).await;
            let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
            let _ = write_frame(&mut server_write1, Opcode::Frame, ready_json).await;
            drop(server_read1);
            drop(server_write1);
        });

        adapter
            .connect_with_io(client_read1, client_write1, 0, &app_id)
            .await?;
        let _ = hs1.await;

        // Give reader task a moment to detect EOF and enqueue Disconnected
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Session 2: reconnect over a fresh duplex stream
        let (client_io2, server_io2) = duplex(4096);
        let (client_read2, client_write2) = tokio::io::split(client_io2);
        let (mut server_read2, mut server_write2) = tokio::io::split(server_io2);

        let hs2 = tokio::spawn(async move {
            let _ = read_frame(&mut server_read2).await;
            let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
            let _ = write_frame(&mut server_write2, Opcode::Frame, ready_json).await;
            // Keep open
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        adapter
            .connect_with_io(client_read2, client_write2, 0, &app_id)
            .await?;

        // Calling next_event with a 50ms timeout should NOT return the stale disconnect from session 1!
        let event_res = tokio::time::timeout(Duration::from_millis(50), adapter.next_event()).await;
        assert!(
            event_res.is_err(),
            "Expected no stale events, but received an event"
        );

        let _ = hs2.await;
        let _ = adapter.disconnect().await;
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_remains_bounded_even_when_writer_mutex_is_held() -> Result<(), DiscordError>
    {
        let (client_io, _server_io) = duplex(64);
        let (_client_read, client_write) = tokio::io::split(client_io);
        let boxed_writer: Box<dyn AsyncWrite + Send + Unpin> = Box::new(client_write);
        let writer_mutex = Arc::new(Mutex::new(boxed_writer));

        // Hold the writer lock on a background task to simulate a stalled Pong write
        let locked_guard = writer_mutex.clone().lock_owned().await;

        let mut adapter = LiveDiscordIpcAdapter::new();
        adapter.writer = Some(writer_mutex);
        adapter.session_info = Some(SessionInfo {
            protocol_version: 1,
            pipe_slot: 0,
            environment: None,
        });

        let start = std::time::Instant::now();
        let res = adapter.disconnect().await;
        let elapsed = start.elapsed();

        assert!(res.is_ok());
        assert!(!adapter.is_connected());
        // Must time out around CLOSE_TIMEOUT (2s), not hang indefinitely
        assert!(elapsed >= Duration::from_millis(1800) && elapsed < Duration::from_secs(4));
        drop(locked_guard);
        Ok(())
    }

    #[tokio::test]
    async fn clear_activity_shared_timeout_enforces_single_2s_budget_and_handles_rpc_errors(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let app_id = ApplicationId::parse("123456789012345678")?;

        // 1. RateLimited (4002) and RpcRejected (5000) responses on clear_activity()
        {
            let (client_io, server_io) = duplex(4096);
            let (client_read, client_write) = tokio::io::split(client_io);
            let (mut server_read, mut server_write) = tokio::io::split(server_io);

            let mut adapter = LiveDiscordIpcAdapter::new();
            let server_task = tokio::spawn(async move {
                let (op, _) = read_frame(&mut server_read).await?;
                assert_eq!(op, Opcode::Handshake);
                let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
                write_frame(&mut server_write, Opcode::Frame, ready_json).await?;

                // First clear -> 4002 RateLimited
                let (op1, bytes1) = read_frame(&mut server_read).await?;
                assert_eq!(op1, Opcode::Frame);
                let obj1: serde_json::Value =
                    serde_json::from_slice(&bytes1).map_err(|e| DiscordError::Io(e.to_string()))?;
                let nonce1 = obj1["nonce"].as_str().unwrap_or("").to_owned();
                let err1 = format!(
                    r#"{{"cmd":"SET_ACTIVITY","evt":"ERROR","nonce":"{nonce1}","data":{{"code":4002,"message":"Rate limit reached","retry_after":12.0}}}}"#
                );
                write_frame(&mut server_write, Opcode::Frame, err1.as_bytes()).await?;

                // Second clear -> 5000 RpcRejected
                let (op2, bytes2) = read_frame(&mut server_read).await?;
                assert_eq!(op2, Opcode::Frame);
                let obj2: serde_json::Value =
                    serde_json::from_slice(&bytes2).map_err(|e| DiscordError::Io(e.to_string()))?;
                let nonce2 = obj2["nonce"].as_str().unwrap_or("").to_owned();
                let err2 = format!(
                    r#"{{"cmd":"SET_ACTIVITY","evt":"ERROR","nonce":"{nonce2}","data":{{"code":5000,"message":"Clear rejected"}}}}"#
                );
                write_frame(&mut server_write, Opcode::Frame, err2.as_bytes()).await?;

                let _ = read_frame(&mut server_read).await;
                Ok::<(), DiscordError>(())
            });

            adapter
                .connect_with_io(client_read, client_write, 0, &app_id)
                .await?;

            assert_eq!(
                adapter.clear_activity().await,
                Err(DiscordError::RateLimited {
                    retry_after_secs: Some(12),
                })
            );
            assert_eq!(
                adapter.clear_activity().await,
                Err(DiscordError::RpcRejected {
                    code: 5000,
                    message: "Clear rejected".to_owned(),
                })
            );

            adapter.disconnect().await?;
            server_task.await??;
        }

        // 2. Shared 2s CLEAR_TIMEOUT across write + read (1100ms write delay + silent server -> ~2s total, not 3.1s)
        {
            let (client_io, server_io) = duplex(4096);
            let (client_read, client_write) = tokio::io::split(client_io);
            let (mut server_read, mut server_write) = tokio::io::split(server_io);
            let (done_tx, done_rx) = oneshot::channel::<()>();

            let mut adapter = LiveDiscordIpcAdapter::new();
            let server_task = tokio::spawn(async move {
                let (op, _) = read_frame(&mut server_read).await?;
                assert_eq!(op, Opcode::Handshake);
                let ready_json = br#"{"cmd":"DISPATCH","evt":"READY","data":{"v":1,"config":{}}}"#;
                write_frame(&mut server_write, Opcode::Frame, ready_json).await?;

                let (op, bytes) = read_frame(&mut server_read).await?;
                assert_eq!(op, Opcode::Frame);
                let obj: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| DiscordError::Io(e.to_string()))?;
                assert!(obj["args"]["activity"].is_null());

                let _ = done_rx.await;
                Ok::<(), DiscordError>(())
            });

            adapter
                .connect_with_io(client_read, client_write, 0, &app_id)
                .await?;

            let writer_mutex = adapter
                .writer
                .as_ref()
                .ok_or("missing writer mutex")?
                .clone();
            let write_guard = writer_mutex.lock_owned().await;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1100)).await;
                drop(write_guard);
            });

            let start = std::time::Instant::now();
            let res = adapter.clear_activity().await;
            let elapsed = start.elapsed();

            assert_eq!(res, Err(DiscordError::ClearTimeout));
            assert!(
                elapsed >= Duration::from_millis(1800) && elapsed < Duration::from_millis(2900),
                "expected shared 2s CLEAR_TIMEOUT budget, got {elapsed:?}"
            );

            let _ = done_tx.send(());
            adapter.disconnect().await?;
            server_task.await??;
        }

        Ok(())
    }
}
