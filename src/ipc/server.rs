//! Unix-socket server: accept loop, per-connection handler, dispatcher trait.
//!
//! One reader task per connection consumes `Request` frames and spawns
//! per-request handler tasks. A single writer task per connection
//! serializes outbound frames (responses + events) so each socket has a
//! deterministic byte order.
//!
//! Subscription lifecycle:
//! * `subscribe { topic }` op → server spawns a forwarder task that
//!   relays Hub events into the writer mpsc, returns a `sub_id` to the
//!   client.
//! * `unsubscribe { sub_id }` op → server aborts that forwarder task.
//! * Connection close → writer drops, forwarders unwind.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::daemon::Op;

use super::hub::{Hub, Topic};
use super::protocol::{Request, Response, RpcError};
use super::wire::{read_frame_with_buf, WireError};

/// Daemon-side handler for typed ops. Implementations live in the
/// daemon binary; tests use a small mock. The per-connection reader in
/// this module parses the inbound op string into [`Op`] before invoking
/// dispatch, so handlers never see an unknown op.
#[async_trait::async_trait]
pub trait Dispatcher: Send + Sync + 'static {
    /// Handle one already-parsed op and return its JSON payload (or a
    /// typed RPC error to ship back to the client).
    ///
    /// # Errors
    ///
    /// Implementations return [`RpcError`] for any failure they want to
    /// surface to the client; the wire layer wraps it in a
    /// [`Response::err`]. Common variants:
    /// - `RpcError::bad_args` for malformed `args`.
    /// - `RpcError::internal` for backend (DB / IO / IMAP / SMTP)
    ///   failures the dispatcher chooses to expose as a generic
    ///   internal error.
    /// - Tool-specific codes set by handlers themselves.
    async fn dispatch(&self, op: Op, args: Value) -> Result<Value, RpcError>;
}

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Hard cap on simultaneously connected clients. Beyond this we accept
/// then immediately close — better than silently queueing.
pub const MAX_CONNECTIONS: usize = 64;

/// Hard cap on subscriptions per connection. The TUI typically has
/// 1-3 (mail.new, mail.updated, mcp.approval_requested).
pub const MAX_SUBS_PER_CONN: usize = 32;

/// Per-connection writer mailbox capacity. Bigger than typical TUI burst.
pub const WRITER_MAILBOX: usize = 128;

/// Handle to a running server. Drop or call `shutdown` to stop the
/// accept loop and close the socket file.
pub struct ServerHandle {
    join: Option<JoinHandle<()>>,
    cancel: CancellationToken,
    path: PathBuf,
}

impl ServerHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Cancel the accept loop, drain in-flight per-connection handshakes,
    /// then remove the socket file.
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.join.take() {
            // Accept loop drains its own JoinSet before returning, so
            // awaiting here also waits for in-flight handshakes.
            if let Err(e) = handle.await {
                tracing::warn!(error = %e, "ipc accept loop join failed");
            }
        }
        // Best-effort cleanup during shutdown; ignore errors so other tasks don't block.
        let _ = tokio::fs::remove_file(&self.path).await;
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Signal the accept loop and any per-connection task observing the
        // token; we can't await in Drop so the runtime will reap detached
        // tasks once they observe the cancel.
        self.cancel.cancel();
        // Best-effort cleanup; we can't await in Drop.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bind a Unix socket at `path` (removing any stale file first), spawn
/// the accept loop, and return a handle.
///
/// # Errors
///
/// Returns [`ServerError::Io`] if the parent directory cannot be
/// created or the `bind(2)` call fails (port-in-use analogue: another
/// process already owns the socket path and removal failed). Stale
/// file cleanup before bind is best-effort; failures there are
/// logged, not surfaced.
pub async fn listen<D: Dispatcher>(
    path: &Path,
    dispatcher: Arc<D>,
    hub: Arc<Hub>,
) -> Result<ServerHandle, ServerError> {
    if path.exists() {
        if let Err(e) = tokio::fs::remove_file(path).await {
            tracing::warn!(?path, error = %e, "could not remove existing socket file");
        }
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let listener = UnixListener::bind(path)?;
    let path_buf = path.to_path_buf();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(accept_loop(listener, dispatcher, hub, cancel.clone()));
    Ok(ServerHandle {
        join: Some(join),
        cancel,
        path: path_buf,
    })
}

async fn accept_loop<D: Dispatcher>(
    listener: UnixListener,
    dispatcher: Arc<D>,
    hub: Arc<Hub>,
    cancel: CancellationToken,
) {
    let conn_count = Arc::new(AtomicU64::new(0));
    let mut tasks: JoinSet<()> = JoinSet::new();
    loop {
        let stream = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            // Reap finished connection tasks so the JoinSet doesn't grow
            // unboundedly during the lifetime of the server. Accept-time
            // gating via MAX_CONNECTIONS still bounds in-flight work.
            Some(res) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(e) = res {
                    if !e.is_cancelled() {
                        tracing::debug!(error = %e, "connection task join failed");
                    }
                }
                continue;
            }
            accept = listener.accept() => match accept {
                Ok((s, _)) => s,
                Err(e) => {
                    tracing::error!(error = %e, "accept failed");
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => continue,
                    }
                }
            },
        };
        let active = conn_count.load(Ordering::Relaxed);
        if active as usize >= MAX_CONNECTIONS {
            tracing::warn!(active, "max connections reached, closing new one");
            drop(stream);
            continue;
        }
        conn_count.fetch_add(1, Ordering::Relaxed);
        let dispatcher = dispatcher.clone();
        let hub = hub.clone();
        let counter = conn_count.clone();
        let conn_cancel = cancel.clone();
        tasks.spawn(async move {
            if let Err(e) = handle_connection(stream, dispatcher, hub, conn_cancel).await {
                tracing::debug!(error = %e, "connection ended with error");
            }
            counter.fetch_sub(1, Ordering::Relaxed);
        });
    }

    // Drain in-flight per-connection handshakes after the cancel signal.
    // Each handle_connection completes naturally once its client closes;
    // awaiting here turns the previous abort-and-orphan behaviour into a
    // cooperative drain.
    while let Some(res) = tasks.join_next().await {
        if let Err(e) = res {
            if !e.is_cancelled() {
                tracing::debug!(error = %e, "connection task join failed during shutdown");
            }
        }
    }
}

/// Outbound frame queued for the per-connection writer task.
///
/// We pass either a fully-encoded response (small, allocated once) or
/// an `EventOut` descriptor. The writer task encodes events into a
/// reusable buffer so a busy subscriber does not allocate a fresh
/// `Vec<u8>` per frame.
enum OutFrame {
    Response(Response),
    Event(EventOut),
}

/// Server-side event payload. Holds the topic as `&'static str`
/// (borrowed from `Topic::as_str`) and the JSON payload as `Arc<Value>`
/// so multiple subscribers share one allocation. Serializes to the same
/// shape as [`super::protocol::Event`].
#[derive(Serialize)]
struct EventOut {
    sub: u64,
    topic: Cow<'static, str>,
    data: Arc<Value>,
}

async fn handle_connection<D: Dispatcher>(
    stream: UnixStream,
    dispatcher: Arc<D>,
    hub: Arc<Hub>,
    cancel: CancellationToken,
) -> Result<(), WireError> {
    let (mut reader, mut writer) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<OutFrame>(WRITER_MAILBOX);

    // Writer task: serializes outbound frames so the wire stays well-ordered.
    // Reuses a single encode buffer across frames; capacity is retained.
    let writer_task = tokio::spawn(async move {
        let mut encode_buf: Vec<u8> = Vec::with_capacity(1024);
        while let Some(frame) = out_rx.recv().await {
            encode_buf.clear();
            let encode_result = match &frame {
                OutFrame::Response(resp) => serde_json::to_writer(&mut encode_buf, resp),
                OutFrame::Event(event) => serde_json::to_writer(&mut encode_buf, event),
            };
            if let Err(e) = encode_result {
                tracing::error!(error = %e, "failed to encode outbound frame");
                continue;
            }
            let len = (encode_buf.len() as u32).to_be_bytes();
            if writer.write_all(&len).await.is_err() {
                break;
            }
            if writer.write_all(&encode_buf).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
    });

    // Per-connection subscription map: sub_id → forwarder JoinHandle.
    let subs: Arc<Mutex<HashMap<u64, JoinHandle<()>>>> = Arc::new(Mutex::new(HashMap::new()));
    let next_sub_id = Arc::new(AtomicU64::new(1));

    // Reader loop. Reuses a single read buffer across inbound frames so
    // a chatty client doesn't allocate per request. The cancel token
    // lets `ServerHandle::shutdown` cooperatively close idle readers
    // instead of orphaning them with `JoinHandle::abort`.
    let mut read_buf: Vec<u8> = Vec::with_capacity(1024);
    let reader_result: Result<(), WireError> = loop {
        let req: Request = tokio::select! {
            biased;
            _ = cancel.cancelled() => break Ok(()),
            frame = read_frame_with_buf(&mut reader, &mut read_buf) => match frame {
                Ok(r) => r,
                Err(WireError::Closed) => break Ok(()),
                Err(e) => break Err(e),
            },
        };

        let dispatcher = dispatcher.clone();
        let hub = hub.clone();
        let out_tx = out_tx.clone();
        let subs = subs.clone();
        let next_sub_id = next_sub_id.clone();

        tokio::spawn(async move {
            let response = match req.op.as_str() {
                "subscribe" => handle_subscribe(&req, &hub, &subs, &next_sub_id, &out_tx).await,
                "unsubscribe" => handle_unsubscribe(&req, &subs).await,
                "ping" => Response::ok(req.id, json!({"pong": true})),
                other => match other.parse::<Op>() {
                    Ok(op) => match dispatcher.dispatch(op, req.args.clone()).await {
                        Ok(data) => Response::ok(req.id, data),
                        Err(err) => Response::err(req.id, err),
                    },
                    Err(_) => Response::err(req.id, RpcError::unknown_op(other)),
                },
            };
            send_response(&out_tx, response).await;
        });
    };

    // Drain: stop accepting new requests, abort all forwarders, drop writer.
    {
        let mut map = subs.lock().await;
        for (_, handle) in map.drain() {
            handle.abort();
        }
    }
    drop(out_tx);
    let _ = writer_task.await;
    reader_result
}

async fn handle_subscribe(
    req: &Request,
    hub: &Hub,
    subs: &Mutex<HashMap<u64, JoinHandle<()>>>,
    next_sub_id: &AtomicU64,
    out_tx: &mpsc::Sender<OutFrame>,
) -> Response {
    let topic_str = match req.args.get("topic").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Response::err(req.id, RpcError::bad_args("missing 'topic'")),
    };
    let topic = match Topic::parse(topic_str) {
        Some(t) => t,
        None => {
            return Response::err(
                req.id,
                RpcError::bad_args(format!("unknown topic '{topic_str}'")),
            )
        }
    };

    {
        let map = subs.lock().await;
        if map.len() >= MAX_SUBS_PER_CONN {
            return Response::err(
                req.id,
                RpcError::new(
                    "too_many_subs",
                    format!("subscription limit ({MAX_SUBS_PER_CONN}) reached"),
                ),
            );
        }
    }

    let sub_id = next_sub_id.fetch_add(1, Ordering::Relaxed);
    let mut rx = hub.subscribe(topic).await;
    let out_tx_fwd = out_tx.clone();
    let topic_name = topic.as_str();

    let handle = tokio::spawn(async move {
        loop {
            let payload: Arc<Value> = match rx.recv().await {
                Ok(p) => p,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    Arc::new(json!({"lagged": n}))
                }
            };
            let event = EventOut {
                sub: sub_id,
                topic: Cow::Borrowed(topic_name),
                data: payload,
            };
            if out_tx_fwd.send(OutFrame::Event(event)).await.is_err() {
                break;
            }
        }
    });

    {
        let mut map = subs.lock().await;
        map.insert(sub_id, handle);
    }
    Response::ok(req.id, json!({"sub_id": sub_id, "topic": topic_str}))
}

async fn handle_unsubscribe(req: &Request, subs: &Mutex<HashMap<u64, JoinHandle<()>>>) -> Response {
    let sub_id = match req.args.get("sub_id").and_then(|v| v.as_u64()) {
        Some(s) => s,
        None => return Response::err(req.id, RpcError::bad_args("missing 'sub_id'")),
    };
    let removed = {
        let mut map = subs.lock().await;
        map.remove(&sub_id)
    };
    match removed {
        Some(handle) => {
            handle.abort();
            Response::ok(req.id, json!({"unsubscribed": true}))
        }
        None => Response::err(
            req.id,
            RpcError::new("unknown_sub", format!("no subscription with id {sub_id}")),
        ),
    }
}

async fn send_response(out_tx: &mpsc::Sender<OutFrame>, resp: Response) {
    // best-effort; the writer task may have shut down if the connection closed.
    let _ = out_tx.send(OutFrame::Response(resp)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::client::Client;
    use std::sync::atomic::AtomicU32;

    /// Test fixture: re-uses two real ops so the wire boundary's
    /// `Op::from_str` accepts them. `AccountList` echoes args; `Search`
    /// fails. Real-op names that aren't otherwise exercised in this
    /// file's tests.
    struct EchoDispatcher {
        calls: AtomicU32,
    }

    #[async_trait::async_trait]
    impl Dispatcher for EchoDispatcher {
        async fn dispatch(&self, op: Op, args: Value) -> Result<Value, RpcError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match op {
                Op::AccountList => Ok(args),
                Op::Search => Err(RpcError::internal("told to fail")),
                other => Err(RpcError::internal(format!("unexpected op: {other}"))),
            }
        }
    }

    struct ServerCtx {
        path: PathBuf,
        _dir: tempfile::TempDir,
        _server: ServerHandle,
        hub: Arc<Hub>,
        dispatcher: Arc<EchoDispatcher>,
    }

    async fn start_server() -> ServerCtx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        let hub = Arc::new(Hub::new());
        let dispatcher = Arc::new(EchoDispatcher {
            calls: AtomicU32::new(0),
        });
        let server = listen(&path, dispatcher.clone(), hub.clone())
            .await
            .unwrap();
        ServerCtx {
            path,
            _dir: dir,
            _server: server,
            hub,
            dispatcher,
        }
    }

    #[tokio::test]
    async fn test_ping_pong() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("ping", json!({})).await.unwrap();
        assert!(resp.ok);
        assert_eq!(resp.data["pong"], true);
    }

    #[tokio::test]
    async fn test_dispatcher_echo_round_trip() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client
            .request("account.list", json!({"a": 1}))
            .await
            .unwrap();
        assert!(resp.ok);
        assert_eq!(resp.data, json!({"a": 1}));
        assert_eq!(ctx.dispatcher.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_dispatcher_error_returns_typed_response() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("search", json!({})).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "internal");
    }

    #[tokio::test]
    async fn test_unknown_op_returns_unknown_op_code() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("frobulate", json!({})).await.unwrap();
        assert!(!resp.ok);
        let err = resp.error.unwrap();
        assert_eq!(err.code, "unknown_op");
        assert!(
            err.message.contains("frobulate"),
            "error message must include offending op, got: {}",
            err.message
        );
    }

    /// CRIT-4 wire-boundary parse: an unknown op never reaches the
    /// dispatcher. Confirms the typed-Op refactor preserves the
    /// pre-existing `unknown_op` error code/message shape byte-for-byte.
    #[tokio::test]
    async fn test_dispatch_unknown_op_returns_rpc_error_at_wire_boundary() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("garbage", json!({})).await.unwrap();
        assert!(!resp.ok);
        let err = resp.error.unwrap();
        assert_eq!(err.code, "unknown_op");
        assert_eq!(err.message, "unknown op 'garbage'");
        // Dispatcher must not have been called for an unknown op —
        // parsing fails at the wire boundary.
        assert_eq!(ctx.dispatcher.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_subscribe_receives_publish() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let sub = client.subscribe(Topic::MailNew).await.unwrap();

        // Give the forwarder a moment to register before publishing.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ctx.hub.publish(Topic::MailNew, json!({"id": "abc"})).await;

        let event = client.next_event().await.unwrap();
        assert_eq!(event.sub, sub);
        assert_eq!(event.topic, "mail.new");
        assert_eq!(event.data, json!({"id": "abc"}));
    }

    #[tokio::test]
    async fn test_unsubscribe_stops_events() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let sub = client.subscribe(Topic::MailNew).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let resp = client
            .request("unsubscribe", json!({ "sub_id": sub }))
            .await
            .unwrap();
        assert!(resp.ok);

        // Allow the abort to land, then publish — no event should arrive.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ctx.hub
            .publish(Topic::MailNew, json!({"after": true}))
            .await;

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(50), client.next_event()).await;
        assert!(result.is_err(), "unsubscribe must stop further events");
    }

    #[tokio::test]
    async fn test_subscribe_with_unknown_topic_errors() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client
            .request("subscribe", json!({"topic": "garbage.value"}))
            .await
            .unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "bad_args");
    }

    #[tokio::test]
    async fn test_pipelined_requests_all_responded_to() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();

        let mut ids = Vec::new();
        for i in 0..20 {
            let id = client
                .send_request("account.list", json!({ "i": i }))
                .await
                .unwrap();
            ids.push(id);
        }
        let mut received = std::collections::HashMap::new();
        for _ in 0..20 {
            let resp = client.next_response().await.unwrap();
            received.insert(resp.id, resp.data);
        }
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(received[id], json!({"i": i}));
        }
    }

    #[tokio::test]
    async fn test_unsubscribe_unknown_id_errors() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client
            .request("unsubscribe", json!({"sub_id": 999}))
            .await
            .unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "unknown_sub");
    }

    #[tokio::test]
    async fn test_subscribe_missing_topic_errors() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("subscribe", json!({})).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "bad_args");
    }

    /// E-H6 regression: shutdown must drive the accept loop to completion
    /// cooperatively (CancellationToken) and drain in-flight per-connection
    /// handshakes (JoinSet) instead of `JoinHandle::abort`-ing them.
    #[tokio::test]
    async fn test_shutdown_drains_in_flight_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shutdown.sock");
        let hub = Arc::new(Hub::new());
        let dispatcher = Arc::new(EchoDispatcher {
            calls: AtomicU32::new(0),
        });
        let server = listen(&path, dispatcher.clone(), hub.clone())
            .await
            .unwrap();

        // Establish a real connection so the server-side per-connection
        // task is mid-handshake (reader awaiting the next frame, writer
        // task idle on its mpsc) when shutdown is signalled.
        let mut client = Client::connect(&path).await.unwrap();
        let resp = client.request("ping", json!({})).await.unwrap();
        assert!(resp.ok);

        // Shutdown must complete without panicking even with an active
        // connection. The accept loop drains its JoinSet before returning.
        tokio::time::timeout(std::time::Duration::from_secs(2), server.shutdown())
            .await
            .expect("graceful shutdown must complete promptly");

        // Socket file is removed.
        assert!(!path.exists(), "socket file should be cleaned up");

        // Existing client observes EOF on its read half once the server
        // drops the connection. New connect attempts fail because the
        // listener is gone.
        let connect = Client::connect(&path).await;
        assert!(connect.is_err(), "no listener after shutdown");
    }
}
