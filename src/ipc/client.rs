//! Client side of the IPC protocol.
//!
//! Used by the TUI, the MCP shim, and tests. Spawns a single reader
//! task that demuxes inbound frames into:
//! - per-`id` `oneshot::Sender<Response>` registered by [`Client::request`]
//! - a single mpsc for [`Client::send_request`] / [`Client::next_response`]
//! - a single mpsc for events
//!
//! Pipelining: `send_request` ships a frame and returns immediately;
//! `next_response` pulls the next response off the mpsc.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use super::hub::Topic;
use super::protocol::{Event, Frame, Request, Response, RpcError};
use super::wire::{read_frame, WireError, MAX_FRAME_BYTES};

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("wire: {0}")]
    Wire(#[from] WireError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection closed")]
    Closed,
    #[error("rpc returned no data: {0}")]
    EmptyResponse(String),
    #[error("server returned error: {code}: {message}")]
    Server { code: String, message: String },
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

pub struct Client {
    writer: OwnedWriteHalf,
    next_id: AtomicU64,
    pending: PendingMap,
    responses_rx: mpsc::Receiver<Response>,
    events_rx: mpsc::Receiver<Event>,
    reader_task: Option<JoinHandle<()>>,
}

impl Client {
    /// Connect to the daemon socket at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Io`] if the `connect(2)` call fails (no
    /// daemon listening, permission denied, broken socket file). All
    /// other variants only fire after the connection is established.
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).await?;
        let (mut reader, writer) = stream.into_split();
        let (events_tx, events_rx) = mpsc::channel(256);
        let (responses_tx, responses_rx) = mpsc::channel(256);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_for_task = pending.clone();

        let reader_task = tokio::spawn(async move {
            loop {
                let frame: Frame = match read_frame(&mut reader).await {
                    Ok(f) => f,
                    Err(_) => break,
                };
                match frame {
                    Frame::Response(r) => {
                        let waker = {
                            let mut map = pending_for_task.lock().await;
                            map.remove(&r.id)
                        };
                        match waker {
                            Some(tx) => {
                                let _ = tx.send(r);
                            }
                            None => {
                                if responses_tx.send(r).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Frame::Event(e) => {
                        if events_tx.send(e).await.is_err() {
                            break;
                        }
                    }
                    Frame::Request(_) => {
                        tracing::warn!("received request frame on client side; ignoring");
                    }
                }
            }
            // Connection closed: drop pending senders so awaiters wake.
            let mut map = pending_for_task.lock().await;
            map.clear();
        });

        Ok(Self {
            writer,
            next_id: AtomicU64::new(1),
            pending,
            responses_rx,
            events_rx,
            reader_task: Some(reader_task),
        })
    }

    /// Send a request, then await the matching response. Concurrent
    /// `request` calls on the same client are fine — each gets its own
    /// oneshot keyed by id.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`ClientError::Json`] if `args` (combined with op metadata)
    ///   cannot be serialised.
    /// - [`ClientError::Wire`] if the encoded frame exceeds
    ///   [`crate::ipc::wire::MAX_FRAME_BYTES`].
    /// - [`ClientError::Io`] if the socket write fails.
    /// - [`ClientError::Closed`] if the reader task exits before the
    ///   matching response arrives (typically the daemon disconnected).
    pub async fn request(&mut self, op: &str, args: Value) -> Result<Response, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }
        let req = Request {
            id,
            op: op.into(),
            args,
        };
        if let Err(e) = self.write_frame(&req).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        rx.await.map_err(|_| ClientError::Closed)
    }

    /// Pipeline-style: ship the request without awaiting. The matching
    /// response will arrive on [`Client::next_response`] in arbitrary
    /// order. Returns the request id.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`ClientError::Json`] if `args` cannot be serialised.
    /// - [`ClientError::Wire`] if the encoded frame exceeds
    ///   [`crate::ipc::wire::MAX_FRAME_BYTES`].
    /// - [`ClientError::Io`] if the socket write fails.
    pub async fn send_request(&mut self, op: &str, args: Value) -> Result<u64, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = Request {
            id,
            op: op.into(),
            args,
        };
        self.write_frame(&req).await?;
        Ok(id)
    }

    /// Pull the next response that wasn't claimed by a `request` call.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Closed`] if the reader task exited (the
    /// daemon disconnected, the wire decoded badly, or the connection
    /// was dropped).
    pub async fn next_response(&mut self) -> Result<Response, ClientError> {
        self.responses_rx.recv().await.ok_or(ClientError::Closed)
    }

    /// Subscribe to a topic. Returns the daemon-allocated `sub_id`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - Any error from [`Client::request`] (`Io`, `Wire`, `Json`, or
    ///   `Closed`) for the underlying RPC.
    /// - [`ClientError::Server`] if the daemon rejects the subscribe
    ///   (e.g. unknown topic, per-connection subscription cap reached).
    /// - [`ClientError::EmptyResponse`] if the daemon's success
    ///   response omits the `sub_id` field — should never happen
    ///   against a current daemon.
    pub async fn subscribe(&mut self, topic: Topic) -> Result<u64, ClientError> {
        let resp = self
            .request("subscribe", json!({"topic": topic.as_str()}))
            .await?;
        if !resp.ok {
            let err = resp.error.unwrap_or_else(|| RpcError {
                code: "unknown".into(),
                message: "subscribe failed".into(),
            });
            return Err(ClientError::Server {
                code: err.code,
                message: err.message,
            });
        }
        resp.data
            .get("sub_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ClientError::EmptyResponse("subscribe missing sub_id".into()))
    }

    /// Pull the next event off the inbound queue.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Closed`] if the reader task exited (the
    /// daemon disconnected or the wire decoded badly).
    pub async fn next_event(&mut self) -> Result<Event, ClientError> {
        self.events_rx.recv().await.ok_or(ClientError::Closed)
    }

    async fn write_frame<T: serde::Serialize>(&mut self, value: &T) -> Result<(), ClientError> {
        let payload = serde_json::to_vec(value)?;
        if payload.len() > MAX_FRAME_BYTES {
            return Err(ClientError::Wire(WireError::FrameTooLarge(payload.len())));
        }
        let len = (payload.len() as u32).to_be_bytes();
        self.writer.write_all(&len).await?;
        self.writer.write_all(&payload).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_task.take() {
            handle.abort();
        }
    }
}
