use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::discord::DiscordError;

/// Hard defensive upper limit on inbound and outbound Discord RPC frame payload size (ADR D-018).
pub const MAX_PAYLOAD_LEN: usize = 65_536; // 64 KiB

/// Exact byte length of the little-endian binary frame header [u32 opcode, u32 payload_len].
pub const HEADER_LEN: usize = 8;

/// Opcodes specified by Discord RPC over IPC (rpc.mdx).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Opcode {
    Handshake = 0,
    Frame = 1,
    Close = 2,
    Ping = 3,
    Pong = 4,
}

impl Opcode {
    pub const fn from_u32(raw: u32) -> Result<Self, DiscordError> {
        match raw {
            0 => Ok(Self::Handshake),
            1 => Ok(Self::Frame),
            2 => Ok(Self::Close),
            3 => Ok(Self::Ping),
            4 => Ok(Self::Pong),
            _ => Err(DiscordError::ProtocolViolation),
        }
    }

    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Encodes an opcode and payload into an 8-byte little-endian header followed by raw payload bytes.
/// Rejects any payload exceeding 64 KiB (`MAX_PAYLOAD_LEN`) with `DiscordError::ProtocolViolation`.
pub fn encode_frame(opcode: Opcode, payload: &[u8]) -> Result<Vec<u8>, DiscordError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(DiscordError::ProtocolViolation);
    }
    let payload_len = u32::try_from(payload.len()).map_err(|_| DiscordError::ProtocolViolation)?;

    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&opcode.as_u32().to_le_bytes());
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Decodes an 8-byte little-endian header, validating the opcode and enforcing the 64 KiB frame guard.
pub fn decode_header(header: &[u8; HEADER_LEN]) -> Result<(Opcode, u32), DiscordError> {
    let mut opcode_bytes = [0u8; 4];
    opcode_bytes.copy_from_slice(&header[0..4]);
    let opcode_raw = u32::from_le_bytes(opcode_bytes);
    let opcode = Opcode::from_u32(opcode_raw)?;

    let mut len_bytes = [0u8; 4];
    len_bytes.copy_from_slice(&header[4..8]);
    let payload_len = u32::from_le_bytes(len_bytes);

    if (payload_len as usize) > MAX_PAYLOAD_LEN {
        return Err(DiscordError::ProtocolViolation);
    }
    Ok((opcode, payload_len))
}

fn is_disconnect_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
    )
}

/// Asynchronously reads a complete length-prefixed frame from an async reader.
/// Enforces the 64 KiB guard before allocating memory for the payload.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<(Opcode, Vec<u8>), DiscordError> {
    let mut header_buf = [0u8; HEADER_LEN];
    match reader.read_exact(&mut header_buf).await {
        Ok(0) => return Err(DiscordError::Disconnected),
        Ok(_) => {}
        Err(e) if is_disconnect_error(&e) => {
            return Err(DiscordError::Disconnected);
        }
        Err(e) => return Err(DiscordError::Io(e.to_string())),
    }

    let (opcode, payload_len) = decode_header(&header_buf)?;
    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 {
        match reader.read_exact(&mut payload).await {
            Ok(_) => {}
            Err(e) if is_disconnect_error(&e) => {
                return Err(DiscordError::Disconnected);
            }
            Err(e) => return Err(DiscordError::Io(e.to_string())),
        }
    }
    Ok((opcode, payload))
}

/// Asynchronously encodes and writes a complete frame to an async writer.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    opcode: Opcode,
    payload: &[u8],
) -> Result<(), DiscordError> {
    let frame = encode_frame(opcode, payload)?;
    match writer.write_all(&frame).await {
        Ok(()) => {}
        Err(e) if is_disconnect_error(&e) => {
            return Err(DiscordError::Disconnected);
        }
        Err(e) => return Err(DiscordError::Io(e.to_string())),
    }
    match writer.flush().await {
        Ok(()) => Ok(()),
        Err(e) if is_disconnect_error(&e) => Err(DiscordError::Disconnected),
        Err(e) => Err(DiscordError::Io(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn opcode_conversion_valid_and_invalid() {
        assert_eq!(Opcode::from_u32(0), Ok(Opcode::Handshake));
        assert_eq!(Opcode::from_u32(1), Ok(Opcode::Frame));
        assert_eq!(Opcode::from_u32(2), Ok(Opcode::Close));
        assert_eq!(Opcode::from_u32(3), Ok(Opcode::Ping));
        assert_eq!(Opcode::from_u32(4), Ok(Opcode::Pong));
        assert_eq!(Opcode::from_u32(5), Err(DiscordError::ProtocolViolation));
        assert_eq!(Opcode::from_u32(999), Err(DiscordError::ProtocolViolation));
    }

    #[test]
    fn encode_frame_builds_correct_header() -> Result<(), DiscordError> {
        let payload = b"{\"v\":1}";
        let frame = encode_frame(Opcode::Handshake, payload)?;

        assert_eq!(frame.len(), 8 + payload.len());
        assert_eq!(&frame[0..4], &0u32.to_le_bytes());
        assert_eq!(&frame[4..8], &(payload.len() as u32).to_le_bytes());
        assert_eq!(&frame[8..], payload);
        Ok(())
    }

    #[test]
    fn encode_frame_rejects_oversized_payload() {
        let oversized = vec![b'a'; MAX_PAYLOAD_LEN + 1];
        assert_eq!(
            encode_frame(Opcode::Frame, &oversized),
            Err(DiscordError::ProtocolViolation)
        );
    }

    #[test]
    fn decode_header_accepts_valid_and_exact_boundary() -> Result<(), DiscordError> {
        // Zero payload length
        let mut header = [0u8; 8];
        header[0..4].copy_from_slice(&1u32.to_le_bytes()); // Frame
        header[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_header(&header)?, (Opcode::Frame, 0));

        // Exactly 64 KiB (65_536 bytes)
        header[4..8].copy_from_slice(&(MAX_PAYLOAD_LEN as u32).to_le_bytes());
        assert_eq!(
            decode_header(&header)?,
            (Opcode::Frame, MAX_PAYLOAD_LEN as u32)
        );
        Ok(())
    }

    #[test]
    fn decode_header_rejects_oversized_payload_len() {
        let mut header = [0u8; 8];
        header[0..4].copy_from_slice(&1u32.to_le_bytes());
        header[4..8].copy_from_slice(&((MAX_PAYLOAD_LEN + 1) as u32).to_le_bytes());
        assert_eq!(decode_header(&header), Err(DiscordError::ProtocolViolation));
    }

    #[test]
    fn decode_header_rejects_invalid_opcode() {
        let mut header = [0u8; 8];
        header[0..4].copy_from_slice(&10u32.to_le_bytes());
        header[4..8].copy_from_slice(&16u32.to_le_bytes());
        assert_eq!(decode_header(&header), Err(DiscordError::ProtocolViolation));
    }

    #[tokio::test]
    async fn async_roundtrip_over_duplex() -> Result<(), DiscordError> {
        let (mut client, mut server) = duplex(1024);
        let payload = b"{\"cmd\":\"SET_ACTIVITY\",\"data\":{}}";

        write_frame(&mut client, Opcode::Frame, payload).await?;

        let (opcode, received) = read_frame(&mut server).await?;
        assert_eq!(opcode, Opcode::Frame);
        assert_eq!(received, payload);
        Ok(())
    }

    #[tokio::test]
    async fn read_frame_handles_eof_gracefully() {
        let (client, mut server) = duplex(64);
        drop(client);

        let res = read_frame(&mut server).await;
        assert_eq!(res, Err(DiscordError::Disconnected));
    }

    #[tokio::test]
    async fn write_frame_returns_disconnected_on_broken_pipe() {
        let (client, mut server) = duplex(64);
        drop(client); // Drops read half; writing to server will fail

        let res = write_frame(&mut server, Opcode::Frame, b"test").await;
        assert_eq!(res, Err(DiscordError::Disconnected));
    }
}
