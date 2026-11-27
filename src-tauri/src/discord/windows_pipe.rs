use crate::discord::DiscordError;

#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// Pipe path pattern used by Discord Desktop on Windows.
pub const DISCORD_PIPE_PREFIX: &str = r"\\.\pipe\discord-ipc-";

/// Bounded slot search range (0 through 9) documented in Discord RPC specifications.
pub const PIPE_SLOT_MIN: u8 = 0;
pub const PIPE_SLOT_MAX: u8 = 9;

#[cfg(windows)]
/// Connects to the first available running Discord desktop IPC named pipe across slots 0..=9.
pub async fn connect_discord_pipe() -> Result<(NamedPipeClient, u8), DiscordError> {
    connect_discord_pipe_in_range(DISCORD_PIPE_PREFIX, PIPE_SLOT_MIN..=PIPE_SLOT_MAX).await
}

#[cfg(windows)]
/// Searches for an active pipe within the specified slot range and prefix pattern.
pub async fn connect_discord_pipe_in_range(
    prefix: &str,
    range: std::ops::RangeInclusive<u8>,
) -> Result<(NamedPipeClient, u8), DiscordError> {
    let mut saw_busy = false;
    for slot in range {
        let pipe_path = format!("{prefix}{slot}");
        match ClientOptions::new().open(&pipe_path) {
            Ok(client) => return Ok((client, slot)),
            Err(e) => {
                // ERROR_PIPE_BUSY (231): pipe instance busy, record and advance.
                if e.raw_os_error() == Some(231) {
                    saw_busy = true;
                }
                // ERROR_FILE_NOT_FOUND (2), ERROR_ACCESS_DENIED (5), and other I/O errors scan next slot.
            }
        }
    }
    if saw_busy {
        Err(DiscordError::PipeBusy)
    } else {
        Err(DiscordError::DiscordNotRunning)
    }
}

#[cfg(not(windows))]
pub struct NamedPipeClient;

#[cfg(not(windows))]
pub async fn connect_discord_pipe() -> Result<(NamedPipeClient, u8), DiscordError> {
    Err(DiscordError::DiscordNotRunning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_prefix_and_slot_bounds_match_specification() {
        assert_eq!(DISCORD_PIPE_PREFIX, r"\\.\pipe\discord-ipc-");
        assert_eq!(PIPE_SLOT_MIN, 0);
        assert_eq!(PIPE_SLOT_MAX, 9);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn connect_pipe_returns_discord_not_running_for_nonexistent_pipe() {
        let res =
            connect_discord_pipe_in_range(r"\\.\pipe\nonexistent-didi-test-pipe-", 0..=2).await;
        assert!(matches!(res, Err(DiscordError::DiscordNotRunning)));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn connect_pipe_returns_pipe_busy_when_all_instances_occupied(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let test_pipe_path = format!(r"\\.\pipe\didi-busy-test-pipe-{}", std::process::id());
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&test_pipe_path)?;

        // First client connects and occupies the single allowed instance
        let _client = ClientOptions::new().open(&test_pipe_path)?;

        // Scanning with our connector should encounter ERROR_PIPE_BUSY (231) and return PipeBusy
        let busy_prefix = format!(r"\\.\pipe\didi-busy-test-pipe-{}", std::process::id());
        let last_byte = busy_prefix.as_bytes().last().copied().unwrap_or(b'0');
        let digit = last_byte - b'0';
        let prefix = &busy_prefix[..busy_prefix.len() - 1];
        let res = connect_discord_pipe_in_range(prefix, digit..=digit).await;
        assert!(matches!(res, Err(DiscordError::PipeBusy)));
        Ok(())
    }
}
