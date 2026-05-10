//! Length-prefixed JSON frame codec.
//!
//! Wire format: `<u32-BE length><JSON payload>`. Length excludes the prefix.
//! Frames are bounded; oversize frames close the connection.

use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single frame. Larger payloads are sent over an
/// out-of-band channel (file path / attachment fetch).
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Error returned by the wire-level frame codec.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WireError {
    /// Underlying transport I/O failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Frame exceeded [`MAX_FRAME_BYTES`].
    #[error("frame too large: {0} bytes (max 4 MiB)")]
    FrameTooLarge(usize),
    /// JSON encode/decode failed.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Length prefix announced a zero-byte payload.
    #[error("empty frame payload")]
    EmptyPayload,
    /// Peer closed the connection cleanly before sending a frame.
    #[error("connection closed")]
    Closed,
}

/// Read one frame and decode it as `T`. Returns `Err(WireError::Closed)`
/// on a clean EOF before the length prefix.
///
/// Allocates a fresh read buffer per call. Hot per-connection reader
/// loops should prefer [`read_frame_with_buf`] to reuse a single
/// allocation across frames.
///
/// # Errors
///
/// Returns:
/// - [`WireError::Closed`] on clean EOF before the length prefix.
/// - [`WireError::Io`] on any other read error from the underlying
///   transport.
/// - [`WireError::EmptyPayload`] if the length prefix is `0`.
/// - [`WireError::FrameTooLarge`] if the prefix exceeds
///   [`MAX_FRAME_BYTES`].
/// - [`WireError::Json`] if the payload is not valid JSON for `T`.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, WireError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut buf = Vec::new();
    read_frame_with_buf(reader, &mut buf).await
}

/// Like [`read_frame`], but reuses `buf` across calls to avoid the
/// per-frame allocation. The buffer is `resize`d in place; capacity is
/// retained between calls.
///
/// # Errors
///
/// Returns:
/// - [`WireError::Closed`] on clean EOF before the length prefix.
/// - [`WireError::Io`] on any other read error from the underlying
///   transport.
/// - [`WireError::EmptyPayload`] if the length prefix is `0`.
/// - [`WireError::FrameTooLarge`] if the prefix exceeds
///   [`MAX_FRAME_BYTES`].
/// - [`WireError::Json`] if the payload is not valid JSON for `T`.
pub async fn read_frame_with_buf<R, T>(reader: &mut R, buf: &mut Vec<u8>) -> Result<T, WireError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(WireError::Closed),
        Err(e) => return Err(WireError::Io(e)),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Err(WireError::EmptyPayload);
    }
    if len > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(len));
    }

    buf.resize(len, 0);
    reader.read_exact(&mut buf[..len]).await?;
    Ok(serde_json::from_slice(&buf[..len])?)
}

/// Encode `value` as JSON and write a single frame. Flushes the writer.
///
/// # Errors
///
/// Returns:
/// - [`WireError::Json`] if `value` cannot be serialised.
/// - [`WireError::FrameTooLarge`] if the encoded payload exceeds
///   [`MAX_FRAME_BYTES`].
/// - [`WireError::Io`] if any of the prefix write, payload write, or
///   flush fails on the underlying transport.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), WireError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(payload.len()));
    }
    let len = (payload.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tokio::io::duplex;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Sample {
        a: u32,
        b: String,
    }

    #[tokio::test]
    async fn test_round_trip_single_frame() {
        let (a, b) = duplex(64 * 1024);
        let (a_r, mut a_w) = tokio::io::split(a);
        let (mut b_r, _b_w) = tokio::io::split(b);

        let payload = Sample {
            a: 7,
            b: "hello".into(),
        };
        write_frame(&mut a_w, &payload).await.unwrap();

        let got: Sample = read_frame(&mut b_r).await.unwrap();
        assert_eq!(got, payload);

        // Drop both halves of side `a` to close the duplex toward `b_r`.
        drop(a_w);
        drop(a_r);

        let err = read_frame::<_, Sample>(&mut b_r).await.unwrap_err();
        assert!(matches!(err, WireError::Closed));
    }

    #[tokio::test]
    async fn test_pipelined_frames_preserve_order() {
        let (a, b) = duplex(64 * 1024);
        let (mut _a_r, mut a_w) = tokio::io::split(a);
        let (mut b_r, mut _b_w) = tokio::io::split(b);

        for i in 0..10u32 {
            write_frame(
                &mut a_w,
                &Sample {
                    a: i,
                    b: format!("v{i}"),
                },
            )
            .await
            .unwrap();
        }
        drop(a_w);

        for i in 0..10u32 {
            let got: Sample = read_frame(&mut b_r).await.unwrap();
            assert_eq!(got.a, i);
            assert_eq!(got.b, format!("v{i}"));
        }
    }

    #[tokio::test]
    async fn test_frame_too_large_on_write() {
        let (a, _b) = duplex(64);
        let (_a_r, mut a_w) = tokio::io::split(a);
        let huge = "x".repeat(MAX_FRAME_BYTES + 1);
        let err = write_frame(&mut a_w, &huge).await.unwrap_err();
        assert!(matches!(err, WireError::FrameTooLarge(_)));
    }

    #[tokio::test]
    async fn test_frame_too_large_on_read() {
        let (a, b) = duplex(64);
        let (_a_r, mut a_w) = tokio::io::split(a);
        let (mut b_r, _b_w) = tokio::io::split(b);

        // Hand-write a length prefix that exceeds the cap.
        let len = ((MAX_FRAME_BYTES + 1) as u32).to_be_bytes();
        a_w.write_all(&len).await.unwrap();
        drop(a_w);

        let err = read_frame::<_, Sample>(&mut b_r).await.unwrap_err();
        assert!(matches!(err, WireError::FrameTooLarge(_)));
    }

    #[tokio::test]
    async fn test_eof_before_prefix_is_closed() {
        let (a, b) = duplex(64);
        drop(a); // close immediately
        let (mut b_r, _b_w) = tokio::io::split(b);
        let err = read_frame::<_, Sample>(&mut b_r).await.unwrap_err();
        assert!(matches!(err, WireError::Closed));
    }

    #[tokio::test]
    async fn test_bad_json_returns_json_error() {
        let (a, b) = duplex(64);
        let (_a_r, mut a_w) = tokio::io::split(a);
        let (mut b_r, _b_w) = tokio::io::split(b);

        let bad = b"{not json";
        let len = (bad.len() as u32).to_be_bytes();
        a_w.write_all(&len).await.unwrap();
        a_w.write_all(bad).await.unwrap();
        drop(a_w);

        let err = read_frame::<_, Sample>(&mut b_r).await.unwrap_err();
        assert!(matches!(err, WireError::Json(_)));
    }
}
