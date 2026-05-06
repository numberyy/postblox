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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

use super::hub::{Hub, Topic};
use super::protocol::{Event, Request, Response, RpcError};
use super::wire::{read_frame, WireError};

/// Daemon-side handler for opaque ops. Implementations live in the
/// daemon binary; tests use a small mock.
#[async_trait::async_trait]
pub trait Dispatcher: Send + Sync + 'static {
    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError>;
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
    path: PathBuf,
}

impl ServerHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Abort the accept loop and remove the socket file.
    pub async fn shutdown(mut self) {
        if let Some(handle) = self.join.take() {
            handle.abort();
            let _ = handle.await;
        }
        let _ = tokio::fs::remove_file(&self.path).await;
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.join.take() {
            handle.abort();
        }
        // Best-effort cleanup; we can't await in Drop.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bind a Unix socket at `path` (removing any stale file first), spawn
/// the accept loop, and return a handle.
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
    let join = tokio::spawn(accept_loop(listener, dispatcher, hub));
    Ok(ServerHandle {
        join: Some(join),
        path: path_buf,
    })
}

async fn accept_loop<D: Dispatcher>(listener: UnixListener, dispatcher: Arc<D>, hub: Arc<Hub>) {
    let conn_count = Arc::new(AtomicU64::new(0));
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                tracing::error!(error = %e, "accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
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
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, dispatcher, hub).await {
                tracing::debug!(error = %e, "connection ended with error");
            }
            counter.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteFrameKind {
    Response,
    Event,
}

struct OutFrame {
    #[allow(dead_code)] // future flow control
    kind: WriteFrameKind,
    bytes: Vec<u8>,
}

async fn handle_connection<D: Dispatcher>(
    stream: UnixStream,
    dispatcher: Arc<D>,
    hub: Arc<Hub>,
) -> Result<(), WireError> {
    let (mut reader, mut writer) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::channel::<OutFrame>(WRITER_MAILBOX);

    // Writer task: serializes outbound frames so the wire stays well-ordered.
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let len = (frame.bytes.len() as u32).to_be_bytes();
            if writer.write_all(&len).await.is_err() {
                break;
            }
            if writer.write_all(&frame.bytes).await.is_err() {
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

    // Reader loop.
    let reader_result: Result<(), WireError> = loop {
        let req: Request = match read_frame(&mut reader).await {
            Ok(r) => r,
            Err(WireError::Closed) => break Ok(()),
            Err(e) => break Err(e),
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
                other => match dispatcher.dispatch(other, req.args.clone()).await {
                    Ok(data) => Response::ok(req.id, data),
                    Err(err) => Response::err(req.id, err),
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
            let payload = match rx.recv().await {
                Ok(p) => (*p).clone(),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    json!({"lagged": n})
                }
            };
            let event = Event {
                sub: sub_id,
                topic: topic_name.into(),
                data: payload,
            };
            let bytes = match serde_json::to_vec(&event) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to encode event");
                    continue;
                }
            };
            if out_tx_fwd
                .send(OutFrame {
                    kind: WriteFrameKind::Event,
                    bytes,
                })
                .await
                .is_err()
            {
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
    let bytes = match serde_json::to_vec(&resp) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "failed to encode response");
            return;
        }
    };
    let _ = out_tx
        .send(OutFrame {
            kind: WriteFrameKind::Response,
            bytes,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::client::Client;
    use std::sync::atomic::AtomicU32;

    struct EchoDispatcher {
        calls: AtomicU32,
    }

    #[async_trait::async_trait]
    impl Dispatcher for EchoDispatcher {
        async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match op {
                "echo" => Ok(args),
                "fail" => Err(RpcError::internal("told to fail")),
                other => Err(RpcError::unknown_op(other)),
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
        let resp = client.request("echo", json!({"a": 1})).await.unwrap();
        assert!(resp.ok);
        assert_eq!(resp.data, json!({"a": 1}));
        assert_eq!(ctx.dispatcher.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_dispatcher_error_returns_typed_response() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("fail", json!({})).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "internal");
    }

    #[tokio::test]
    async fn test_unknown_op_returns_unknown_op_code() {
        let ctx = start_server().await;
        let mut client = Client::connect(&ctx.path).await.unwrap();
        let resp = client.request("frobulate", json!({})).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, "unknown_op");
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
                .send_request("echo", json!({ "i": i }))
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
}
