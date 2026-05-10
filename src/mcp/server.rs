use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

use crate::ipc::client::{Client, ClientError};
use crate::ipc::{Event, Topic};
use crate::models::{ApprovalState, McpApproval, McpGate};

use super::gates::{self, GateDecision};
use super::protocol::{self, Incoming, JsonRpcError};
use super::tools::{self, ToolSpec};

const NOTIFICATION_METHOD: &str = "notifications/postblox/event";
const NOTIFICATION_ERROR_METHOD: &str = "notifications/postblox/error";
const OUTPUT_MAILBOX: usize = 128;
const _: () = assert!(OUTPUT_MAILBOX == 128);
const SUBSCRIPTION_MAILBOX: usize = 256;
const _: () = assert!(SUBSCRIPTION_MAILBOX == 256);
const STDIO_MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ipc: {0}")]
    Ipc(String),
    #[error("daemon returned error: {code}: {message}")]
    Daemon { code: String, message: String },
    #[error("bad daemon response: {0}")]
    BadDaemonResponse(String),
}

impl BridgeError {
    fn to_json_rpc(&self) -> JsonRpcError {
        match self {
            Self::Daemon { code, message } => JsonRpcError::server(code.clone(), message.clone()),
            Self::BadDaemonResponse(message) => JsonRpcError::internal(message.clone()),
            Self::Io(error) => JsonRpcError::internal(error.to_string()),
            Self::Json(error) => JsonRpcError::internal(error.to_string()),
            Self::Ipc(message) => JsonRpcError::server("ipc_error", message.clone()),
        }
    }
}

impl From<ClientError> for BridgeError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Server { code, message } => Self::Daemon { code, message },
            other => Self::Ipc(other.to_string()),
        }
    }
}

#[async_trait::async_trait]
pub trait DaemonBridge: Send + Sync + 'static {
    async fn request(&self, op: &str, args: Value) -> Result<Value, BridgeError>;
    async fn subscribe(&self, topic: Topic) -> Result<mpsc::Receiver<Event>, BridgeError>;
}

pub struct IpcDaemon {
    socket_path: PathBuf,
    client: Mutex<Client>,
}

impl IpcDaemon {
    pub async fn connect(socket_path: PathBuf) -> Result<Self, BridgeError> {
        let client = Client::connect(&socket_path).await?;
        Ok(Self {
            socket_path,
            client: Mutex::new(client),
        })
    }
}

#[async_trait::async_trait]
impl DaemonBridge for IpcDaemon {
    async fn request(&self, op: &str, args: Value) -> Result<Value, BridgeError> {
        let mut client = self.client.lock().await;
        let response = client.request(op, args).await?;
        if response.ok {
            Ok(response.data)
        } else {
            let error = response.error.unwrap_or_else(|| {
                crate::ipc::RpcError::internal("daemon returned an empty error")
            });
            Err(BridgeError::Daemon {
                code: error.code,
                message: error.message,
            })
        }
    }

    async fn subscribe(&self, topic: Topic) -> Result<mpsc::Receiver<Event>, BridgeError> {
        let mut client = Client::connect(&self.socket_path).await?;
        client.subscribe(topic).await?;
        let (tx, rx) = mpsc::channel(SUBSCRIPTION_MAILBOX);
        tokio::spawn(async move {
            while let Ok(event) = client.next_event().await {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub approval_timeout: Duration,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            approval_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Clone)]
pub struct McpBridge {
    daemon: Arc<dyn DaemonBridge>,
    config: BridgeConfig,
}

impl McpBridge {
    pub fn new(daemon: Arc<dyn DaemonBridge>, config: BridgeConfig) -> Self {
        Self { daemon, config }
    }

    pub async fn handle_line(&self, line: &str) -> Option<Value> {
        match protocol::parse_line(line) {
            Ok(incoming) => self.handle_incoming(incoming).await,
            Err(error_response) => Some(error_response),
        }
    }

    pub async fn handle_incoming(&self, incoming: Incoming) -> Option<Value> {
        match incoming {
            Incoming::Request { id, method, params } => {
                Some(self.handle_request(id, &method, params).await)
            }
            Incoming::Notification { .. } | Incoming::Response => None,
        }
    }

    pub async fn start_notifications(&self, out_tx: mpsc::Sender<Value>) -> Vec<JoinHandle<()>> {
        let mut handles = Vec::new();
        for topic in notification_topics() {
            match self.daemon.subscribe(topic).await {
                Ok(mut rx) => {
                    let out_tx = out_tx.clone();
                    handles.push(tokio::spawn(async move {
                        while let Some(event) = rx.recv().await {
                            let message = protocol::notification(
                                NOTIFICATION_METHOD,
                                json!({
                                    "topic": event.topic,
                                    "data": event.data,
                                }),
                            );
                            if out_tx.send(message).await.is_err() {
                                break;
                            }
                        }
                    }));
                }
                Err(error) => {
                    // best-effort fan-out; subscribers may have unsubscribed since the lookup.
                    let _ = out_tx
                        .send(protocol::notification(
                            NOTIFICATION_ERROR_METHOD,
                            json!({
                                "topic": topic.as_str(),
                                "error": error.to_string(),
                            }),
                        ))
                        .await;
                }
            }
        }
        handles
    }

    async fn handle_request(&self, id: Value, method: &str, params: Value) -> Value {
        let result = match method {
            "initialize" => self.handle_initialize(params),
            "tools/list" => Ok(tools::list_tools()),
            "tools/call" => self.handle_tool_call(params).await,
            "ping" => Ok(json!({})),
            other => Err(JsonRpcError::method_not_found(other)),
        };

        match result {
            Ok(result) => protocol::success_response(id, result),
            Err(error) => protocol::error_response(id, error),
        }
    }

    fn handle_initialize(&self, params: Value) -> Result<Value, JsonRpcError> {
        let params = protocol::object_params(params)?;
        let client_protocol = params.get("protocolVersion").and_then(Value::as_str);
        Ok(protocol::initialize_result(client_protocol))
    }

    async fn handle_tool_call(&self, params: Value) -> Result<Value, JsonRpcError> {
        let params = protocol::object_params(params)?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError::invalid_params("tools/call missing 'name'"))?;
        let tool = tools::find_tool(name)
            .ok_or_else(|| JsonRpcError::invalid_params(format!("unknown tool '{name}'")))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments =
            tools::validate_arguments(tool, arguments).map_err(JsonRpcError::invalid_params)?;

        let result = self
            .call_tool(tool, arguments)
            .await
            .map_err(|error| error.to_json_rpc())?;
        Ok(tool_result(result))
    }

    async fn call_tool(&self, tool: &ToolSpec, arguments: Value) -> Result<Value, BridgeError> {
        if !tool.dangerous {
            return self.daemon.request(tool.op.as_str(), arguments).await;
        }

        match self.gate_decision(tool, &arguments).await? {
            GateDecision::AutoAllow { .. } => self.forward_dangerous_tool(tool, arguments).await,
            GateDecision::Deny { .. } => Err(BridgeError::Daemon {
                code: "gate_denied".into(),
                message: format!("MCP gate denied {}", tool.name),
            }),
            GateDecision::Require { .. } => {
                self.call_after_required_approval(tool, arguments).await
            }
        }
    }

    async fn gate_decision(
        &self,
        tool: &ToolSpec,
        arguments: &Value,
    ) -> Result<GateDecision, BridgeError> {
        let gates = self
            .daemon
            .request("mcp.gate.list", json!({ "tool": tool.name }))
            .await?;
        let gates: Vec<McpGate> = serde_json::from_value(gates)
            .map_err(|e| BridgeError::BadDaemonResponse(format!("mcp.gate.list: {e}")))?;
        Ok(gates::decide(&gates, arguments))
    }

    async fn call_after_required_approval(
        &self,
        tool: &ToolSpec,
        arguments: Value,
    ) -> Result<Value, BridgeError> {
        let approval = self.create_approval(tool, &arguments).await?;
        let state = self.wait_for_approval(approval.id).await?;
        match state {
            ApprovalState::Allowed => self.forward_dangerous_tool(tool, arguments).await,
            ApprovalState::Denied => Err(BridgeError::Daemon {
                code: "approval_denied".into(),
                message: format!("MCP approval {} was denied", approval.id),
            }),
            ApprovalState::Expired => Err(BridgeError::Daemon {
                code: "approval_expired".into(),
                message: format!("MCP approval {} expired", approval.id),
            }),
            ApprovalState::Pending => Err(BridgeError::Daemon {
                code: "approval_timeout".into(),
                message: format!("MCP approval {} timed out", approval.id),
            }),
        }
    }

    async fn create_approval(
        &self,
        tool: &ToolSpec,
        arguments: &Value,
    ) -> Result<McpApproval, BridgeError> {
        let value = self
            .daemon
            .request(
                "mcp.approval.create",
                json!({
                    "tool": tool.name,
                    "args": arguments,
                    "summary": approval_summary(tool, arguments),
                    "_actor": format!("mcp:{}", tool.name),
                }),
            )
            .await?;
        serde_json::from_value(value)
            .map_err(|e| BridgeError::BadDaemonResponse(format!("mcp.approval.create: {e}")))
    }

    async fn wait_for_approval(&self, id: Uuid) -> Result<ApprovalState, BridgeError> {
        // Subscribe before checking current state so a decision published
        // between the check and the wait cannot be missed.
        let mut events = self.daemon.subscribe(Topic::McpApprovalDecided).await?;

        // Catch decisions made before the approval was created or between
        // create and subscribe (e.g. an out-of-band decide).
        let approval = self.get_approval(id).await?;
        if approval.state != ApprovalState::Pending {
            return Ok(approval.state);
        }

        let timeout = sleep(self.config.approval_timeout);
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                biased;
                _ = &mut timeout => {
                    return self.expire_approval(id).await;
                }
                event = events.recv() => {
                    match event {
                        Some(event) => {
                            if let Some(state) = match_decided_event(id, &event) {
                                return Ok(state);
                            }
                            // Either a decision for a different approval, or a
                            // "lagged" notice from the IPC layer; on lag we
                            // re-fetch so we don't miss our own decision.
                            if event_is_lagged(&event) {
                                let approval = self.get_approval(id).await?;
                                if approval.state != ApprovalState::Pending {
                                    return Ok(approval.state);
                                }
                            }
                        }
                        None => {
                            // Subscription dropped; fall back to one DB read so
                            // we don't busy-loop. Pending here means the daemon
                            // is gone — surface the current state.
                            return Ok(self.get_approval(id).await?.state);
                        }
                    }
                }
            }
        }
    }

    async fn expire_approval(&self, id: Uuid) -> Result<ApprovalState, BridgeError> {
        let value = self
            .daemon
            .request(
                "mcp.approval.decide",
                json!({
                    "id": id,
                    "state": "expired",
                    "decided_by": "mcp:timeout",
                }),
            )
            .await?;
        if value
            .get("decided")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(ApprovalState::Expired);
        }
        Ok(self.get_approval(id).await?.state)
    }

    async fn get_approval(&self, id: Uuid) -> Result<McpApproval, BridgeError> {
        let value = self
            .daemon
            .request("mcp.approval.get", json!({ "id": id }))
            .await?;
        if value.is_null() {
            return Err(BridgeError::BadDaemonResponse(format!(
                "approval {id} not found"
            )));
        }
        serde_json::from_value(value)
            .map_err(|e| BridgeError::BadDaemonResponse(format!("mcp.approval.get: {e}")))
    }

    async fn forward_dangerous_tool(
        &self,
        tool: &ToolSpec,
        mut arguments: Value,
    ) -> Result<Value, BridgeError> {
        if let Value::Object(object) = &mut arguments {
            object.insert("_actor".into(), json!(format!("mcp:{}", tool.name)));
        }
        self.daemon.request(tool.op.as_str(), arguments).await
    }
}

pub async fn run_stdio(socket_path: PathBuf) -> Result<(), BridgeError> {
    let daemon = Arc::new(IpcDaemon::connect(socket_path).await?);
    serve_stdio(McpBridge::new(daemon, BridgeConfig::default())).await
}

pub async fn serve_stdio(bridge: McpBridge) -> Result<(), BridgeError> {
    serve_stdio_io_with_limit(
        bridge,
        tokio::io::stdin(),
        tokio::io::stdout(),
        STDIO_MAX_LINE_BYTES,
    )
    .await
}

async fn serve_stdio_io_with_limit<R, W>(
    bridge: McpBridge,
    input: R,
    output: W,
    max_line_bytes: usize,
) -> Result<(), BridgeError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(input);
    let (out_tx, mut out_rx) = mpsc::channel::<Value>(OUTPUT_MAILBOX);

    let writer = tokio::spawn(async move {
        let mut stdout = output;
        while let Some(message) = out_rx.recv().await {
            let bytes = serde_json::to_vec(&message)?;
            stdout.write_all(&bytes).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Ok::<(), BridgeError>(())
    });

    let mut notification_handles: Vec<JoinHandle<()>> = Vec::new();
    while let Some(frame) = read_bounded_line(&mut reader, max_line_bytes).await? {
        match frame {
            StdioFrame::Line(line) => {
                let starts_notifications = is_initialized_notification(&line);
                if let Some(response) = bridge.handle_line(&line).await {
                    if out_tx.send(response).await.is_err() {
                        break;
                    }
                }
                if starts_notifications && notification_handles.is_empty() {
                    notification_handles = bridge.start_notifications(out_tx.clone()).await;
                }
            }
            StdioFrame::Oversized => {
                if out_tx
                    .send(oversized_input_response(max_line_bytes))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }

    for handle in notification_handles {
        handle.abort();
    }
    drop(out_tx);
    writer
        .await
        .map_err(|e| BridgeError::Ipc(e.to_string()))??;
    Ok(())
}

enum StdioFrame {
    Line(String),
    Oversized,
}

async fn read_bounded_line<R>(
    reader: &mut R,
    max_line_bytes: usize,
) -> std::io::Result<Option<StdioFrame>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return decode_stdio_line(line).map(Some);
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline > max_line_bytes {
                reader.consume(newline + 1);
                return Ok(Some(StdioFrame::Oversized));
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            return decode_stdio_line(line).map(Some);
        }

        let available_len = available.len();
        if line.len() + available_len > max_line_bytes {
            reader.consume(available_len);
            discard_until_newline(reader).await?;
            return Ok(Some(StdioFrame::Oversized));
        }
        line.extend_from_slice(available);
        reader.consume(available_len);
    }
}

async fn discard_until_newline<R>(reader: &mut R) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(());
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(newline + 1);
            return Ok(());
        }
        let available_len = available.len();
        reader.consume(available_len);
    }
}

fn decode_stdio_line(mut line: Vec<u8>) -> std::io::Result<StdioFrame> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(StdioFrame::Line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn oversized_input_response(max_line_bytes: usize) -> Value {
    protocol::error_response(
        Value::Null,
        JsonRpcError::invalid_request(format!("input line exceeds {max_line_bytes} bytes")),
    )
}

fn is_initialized_notification(line: &str) -> bool {
    matches!(
        protocol::parse_line(line),
        Ok(Incoming::Notification { method, .. }) if is_initialized_method(&method)
    )
}

fn is_initialized_method(method: &str) -> bool {
    method == "initialized" || method == "notifications/initialized"
}

fn notification_topics() -> [Topic; 5] {
    [
        Topic::MailNew,
        Topic::MailUpdated,
        Topic::AccountSynced,
        Topic::McpApprovalRequested,
        Topic::McpApprovalDecided,
    ]
}

/// Returns the decided state if `event` is an `mcp.approval_decided` payload
/// for the given approval id and carries a parseable `state` field.
fn match_decided_event(id: Uuid, event: &Event) -> Option<ApprovalState> {
    let payload = event.data.as_object()?;
    let event_id = payload.get("approval_id").and_then(Value::as_str)?;
    if event_id.parse::<Uuid>().ok()? != id {
        return None;
    }
    let state = payload.get("state").and_then(Value::as_str)?;
    state.parse::<ApprovalState>().ok()
}

/// The IPC layer surfaces a dropped-broadcast notice as `{"lagged": n}` —
/// when we see it on the approval topic we re-fetch our own approval to
/// avoid missing the decision.
fn event_is_lagged(event: &Event) -> bool {
    event
        .data
        .as_object()
        .is_some_and(|m| m.contains_key("lagged"))
}

fn tool_result(value: Value) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
            }
        ],
        "structuredContent": value,
    })
}

fn approval_summary(tool: &ToolSpec, arguments: &Value) -> String {
    let mut fields = HashMap::new();
    if let Some(object) = arguments.as_object() {
        for key in ["id", "account_id", "draft_id", "subject"] {
            if let Some(value) = object.get(key) {
                fields.insert(key, value.clone());
            }
        }
    }
    if fields.is_empty() {
        format!("MCP tool {} requested", tool.name)
    } else {
        format!("MCP tool {} requested with {}", tool.name, json!(fields))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct MockDaemon {
        gates: StdMutex<Vec<McpGate>>,
        calls: StdMutex<Vec<(String, Value)>>,
        approval_get_state: StdMutex<ApprovalState>,
        approval_id: Uuid,
        event_senders: StdMutex<HashMap<Topic, mpsc::Sender<Event>>>,
        forward_response: Value,
    }

    impl MockDaemon {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                gates: StdMutex::new(vec![]),
                calls: StdMutex::new(vec![]),
                approval_get_state: StdMutex::new(ApprovalState::Allowed),
                approval_id: Uuid::new_v4(),
                event_senders: StdMutex::new(HashMap::new()),
                forward_response: json!({"ok": true}),
            })
        }

        fn set_gates(&self, gates: Vec<McpGate>) {
            *self.gates.lock().unwrap() = gates;
        }

        fn set_approval_get_state(&self, state: ApprovalState) {
            *self.approval_get_state.lock().unwrap() = state;
        }

        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }

        async fn emit(&self, topic: Topic, data: Value) {
            let sender = self.event_senders.lock().unwrap().get(&topic).cloned();
            if let Some(sender) = sender {
                sender
                    .send(Event {
                        sub: 1,
                        topic: topic.as_str().into(),
                        data,
                    })
                    .await
                    .unwrap();
            }
        }
    }

    #[async_trait::async_trait]
    impl DaemonBridge for MockDaemon {
        async fn request(&self, op: &str, args: Value) -> Result<Value, BridgeError> {
            self.calls
                .lock()
                .unwrap()
                .push((op.to_string(), args.clone()));
            match op {
                "mcp.gate.list" => {
                    let tool = args.get("tool").and_then(Value::as_str);
                    let gates = self
                        .gates
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|gate| tool.map(|tool| gate.tool == tool).unwrap_or(true))
                        .cloned()
                        .collect::<Vec<_>>();
                    Ok(serde_json::to_value(gates).unwrap())
                }
                "mcp.approval.create" => Ok(serde_json::to_value(self.approval(
                    args["tool"].as_str().unwrap(),
                    args["args"].clone(),
                    ApprovalState::Pending,
                ))
                .unwrap()),
                "mcp.approval.get" => {
                    let state = *self.approval_get_state.lock().unwrap();
                    Ok(serde_json::to_value(self.approval(
                        "postblox_draft_delete",
                        json!({}),
                        state,
                    ))
                    .unwrap())
                }
                "mcp.approval.decide" => {
                    let state = args["state"].as_str().unwrap().parse().unwrap();
                    *self.approval_get_state.lock().unwrap() = state;
                    Ok(json!({ "decided": true }))
                }
                _ => Ok(self.forward_response.clone()),
            }
        }

        async fn subscribe(&self, topic: Topic) -> Result<mpsc::Receiver<Event>, BridgeError> {
            let (tx, rx) = mpsc::channel(SUBSCRIPTION_MAILBOX);
            self.event_senders.lock().unwrap().insert(topic, tx);
            Ok(rx)
        }
    }

    impl MockDaemon {
        fn approval(&self, tool: &str, args: Value, state: ApprovalState) -> McpApproval {
            McpApproval {
                id: self.approval_id,
                tool: tool.into(),
                args,
                summary: "summary".into(),
                state,
                decided_at: None,
                decided_by: None,
                created_at: Utc::now(),
            }
        }
    }

    fn bridge(mock: Arc<MockDaemon>) -> McpBridge {
        McpBridge::new(
            mock,
            BridgeConfig {
                approval_timeout: Duration::from_millis(50),
            },
        )
    }

    fn gate(tool: &str, pattern: Option<&str>, action: crate::models::GateAction) -> McpGate {
        McpGate {
            id: Uuid::new_v4(),
            tool: tool.into(),
            arg_pattern: pattern.map(str::to_string),
            action,
            note: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_initialize_tools_list_and_read_tool_forwarding() {
        let mock = MockDaemon::new();
        let bridge = bridge(mock.clone());

        let init = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "postblox-mcp");

        let listed = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .unwrap();
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 14);

        let called = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"postblox_account_list","arguments":{}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(called["result"]["structuredContent"], json!({"ok": true}));
        assert_eq!(mock.calls(), vec![("account.list".into(), json!({}))]);
    }

    #[tokio::test]
    async fn test_unknown_method_and_bad_tool_call_return_json_rpc_errors() {
        let bridge = bridge(MockDaemon::new());
        let unknown = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#)
            .await
            .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);

        let bad = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}"#)
            .await
            .unwrap();
        assert_eq!(bad["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn test_gate_auto_allow_forwards_dangerous_tool_with_mcp_actor() {
        let mock = MockDaemon::new();
        mock.set_gates(vec![gate(
            "postblox_draft_delete",
            None,
            crate::models::GateAction::AutoAllow,
        )]);
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert!(response.get("error").is_none(), "{response:?}");
        let calls = mock.calls();
        assert_eq!(calls[0].0, "mcp.gate.list");
        assert_eq!(calls[1].0, "draft.delete");
        assert_eq!(calls[1].1["_actor"], "mcp:postblox_draft_delete");
    }

    #[tokio::test]
    async fn test_invalid_flags_rejected_before_gate_lookup() {
        let mock = MockDaemon::new();
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_message_set_flags","arguments":{"id":"00000000-0000-0000-0000-000000000001","flags":"\\Seen"}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(
            response["error"]["message"],
            "flags must be an array of strings"
        );
        assert!(mock.calls().is_empty());
    }

    #[tokio::test]
    async fn test_partial_draft_update_rejected_before_gate_lookup() {
        let mock = MockDaemon::new();
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_update","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(
            response["error"]["message"],
            "missing required argument 'to_addrs'"
        );
        assert!(mock.calls().is_empty());
    }

    #[tokio::test]
    async fn test_gate_deny_rejects_without_forwarding_dangerous_op() {
        let mock = MockDaemon::new();
        mock.set_gates(vec![gate(
            "postblox_draft_delete",
            None,
            crate::models::GateAction::Deny,
        )]);
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "gate_denied");
        assert_eq!(
            mock.calls()
                .iter()
                .filter(|(op, _)| op == "draft.delete")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn test_required_approval_allowed_then_forwards() {
        let mock = MockDaemon::new();
        mock.set_approval_get_state(ApprovalState::Allowed);
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert!(response.get("error").is_none(), "{response:?}");
        let ops = mock
            .calls()
            .into_iter()
            .map(|(op, _)| op)
            .collect::<Vec<_>>();
        assert_eq!(
            ops,
            vec![
                "mcp.gate.list",
                "mcp.approval.create",
                "mcp.approval.get",
                "draft.delete"
            ]
        );
    }

    #[tokio::test]
    async fn test_required_approval_denied_does_not_forward() {
        let mock = MockDaemon::new();
        mock.set_approval_get_state(ApprovalState::Denied);
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "approval_denied");
        assert!(!mock.calls().iter().any(|(op, _)| op == "draft.delete"));
    }

    #[tokio::test]
    async fn test_required_approval_timeout_expires_without_forwarding() {
        let mock = MockDaemon::new();
        mock.set_approval_get_state(ApprovalState::Pending);
        let bridge = bridge(mock.clone());

        let response = bridge
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
            )
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "approval_expired");
        let ops = mock
            .calls()
            .into_iter()
            .map(|(op, _)| op)
            .collect::<Vec<_>>();
        assert!(ops.contains(&"mcp.approval.decide".to_string()));
        assert!(!ops.contains(&"draft.delete".to_string()));
    }

    #[tokio::test]
    async fn test_required_approval_broadcast_wakes_waiter_before_timeout() {
        // Approval starts pending; we simulate a decision arriving via the
        // broadcast subscription, not via DB polling. The timeout is set far
        // higher than the test runtime, so any non-broadcast wakeup would
        // either time out or never arrive.
        let mock = MockDaemon::new();
        mock.set_approval_get_state(ApprovalState::Pending);
        let bridge = McpBridge::new(
            mock.clone(),
            BridgeConfig {
                approval_timeout: Duration::from_secs(30),
            },
        );
        let approval_id = mock.approval_id;

        let bridge_for_task = bridge.clone();
        let waiter = tokio::spawn(async move {
            bridge_for_task
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
                )
                .await
                .unwrap()
        });

        // Wait until wait_for_approval has subscribed.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if mock
                .event_senders
                .lock()
                .unwrap()
                .contains_key(&Topic::McpApprovalDecided)
            {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("subscriber never registered");
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        let started = tokio::time::Instant::now();
        mock.emit(
            Topic::McpApprovalDecided,
            json!({
                "approval_id": approval_id,
                "tool": "postblox_draft_delete",
                "state": "allowed",
                "decided_by": "user:test",
            }),
        )
        .await;

        let response = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("waiter did not return — broadcast wakeup failed")
            .unwrap();
        assert!(response.get("error").is_none(), "{response:?}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "broadcast wakeup took {:?}",
            started.elapsed()
        );

        let ops = mock
            .calls()
            .into_iter()
            .map(|(op, _)| op)
            .collect::<Vec<_>>();
        // The forward must run after the broadcast resolved the wait.
        assert!(ops.contains(&"draft.delete".to_string()));
        // No timeout-driven decide should have been issued.
        assert!(!ops.contains(&"mcp.approval.decide".to_string()));
    }

    #[tokio::test]
    async fn test_required_approval_broadcast_for_other_id_is_ignored() {
        // A decision for a different approval id must NOT wake our waiter.
        let mock = MockDaemon::new();
        mock.set_approval_get_state(ApprovalState::Pending);
        let bridge = McpBridge::new(
            mock.clone(),
            BridgeConfig {
                approval_timeout: Duration::from_millis(80),
            },
        );

        let bridge_for_task = bridge.clone();
        let waiter = tokio::spawn(async move {
            bridge_for_task
                .handle_line(
                    r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"postblox_draft_delete","arguments":{"id":"00000000-0000-0000-0000-000000000001"}}}"#,
                )
                .await
                .unwrap()
        });

        // Wait until subscribed.
        for _ in 0..200 {
            if mock
                .event_senders
                .lock()
                .unwrap()
                .contains_key(&Topic::McpApprovalDecided)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        // Emit a decision for a different approval id.
        mock.emit(
            Topic::McpApprovalDecided,
            json!({
                "approval_id": Uuid::new_v4(),
                "tool": "postblox_draft_delete",
                "state": "allowed",
                "decided_by": "user:test",
            }),
        )
        .await;

        let response = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        // Wrong-id event was ignored; the wait expired via timeout instead.
        assert_eq!(response["error"]["data"]["code"], "approval_expired");
    }

    #[test]
    fn test_match_decided_event_filters_by_approval_id_and_parses_state() {
        let want = Uuid::new_v4();
        let other = Uuid::new_v4();
        let event = |id: Uuid, state: &str| Event {
            sub: 1,
            topic: Topic::McpApprovalDecided.as_str().into(),
            data: json!({"approval_id": id, "state": state}),
        };
        assert_eq!(
            match_decided_event(want, &event(want, "allowed")),
            Some(ApprovalState::Allowed)
        );
        assert_eq!(
            match_decided_event(want, &event(want, "denied")),
            Some(ApprovalState::Denied)
        );
        assert_eq!(match_decided_event(want, &event(other, "allowed")), None);
        let bad_state = Event {
            sub: 1,
            topic: Topic::McpApprovalDecided.as_str().into(),
            data: json!({"approval_id": want, "state": "garbage"}),
        };
        assert_eq!(match_decided_event(want, &bad_state), None);
    }

    #[test]
    fn test_event_is_lagged_only_matches_lagged_payload() {
        let lagged = Event {
            sub: 1,
            topic: Topic::McpApprovalDecided.as_str().into(),
            data: json!({"lagged": 7}),
        };
        let normal = Event {
            sub: 1,
            topic: Topic::McpApprovalDecided.as_str().into(),
            data: json!({"approval_id": Uuid::new_v4(), "state": "allowed"}),
        };
        assert!(event_is_lagged(&lagged));
        assert!(!event_is_lagged(&normal));
    }

    #[tokio::test]
    async fn test_notifications_forward_daemon_events_as_json_rpc_notifications() {
        let mock = MockDaemon::new();
        let bridge = bridge(mock.clone());
        let (tx, mut rx) = mpsc::channel(OUTPUT_MAILBOX);
        let handles = bridge.start_notifications(tx).await;

        mock.emit(Topic::MailNew, json!({"id": "m1"})).await;
        let message = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message["method"], NOTIFICATION_METHOD);
        assert_eq!(message["params"]["topic"], "mail.new");
        assert_eq!(message["params"]["data"], json!({"id": "m1"}));

        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_initialized_notification_gets_no_response() {
        let bridge = bridge(MockDaemon::new());
        let response = bridge
            .handle_line(r#"{"jsonrpc":"2.0","method":"initialized"}"#)
            .await;
        assert!(response.is_none());
    }

    #[test]
    fn test_stdio_framing_uses_single_line_json() {
        let response = tool_result(json!({"body": "hello\nworld"}));
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains('\n'));
        assert!(encoded.contains(r#"hello\nworld"#));
    }

    #[tokio::test]
    async fn test_stdio_rejects_oversized_input_with_json_rpc_error() {
        let bridge = bridge(MockDaemon::new());
        let (mut input_tx, input_rx) = tokio::io::duplex(64);
        let (output_tx, mut output_rx) = tokio::io::duplex(1024);

        let server = tokio::spawn(serve_stdio_io_with_limit(bridge, input_rx, output_tx, 16));
        input_tx.write_all(b"0123456789abcdefg\n").await.unwrap();
        drop(input_tx);

        let mut output = Vec::new();
        output_rx.read_to_end(&mut output).await.unwrap();
        server.await.unwrap().unwrap();

        let line = std::str::from_utf8(&output).unwrap().trim_end();
        let response: Value = serde_json::from_str(line).unwrap();
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["error"]["message"], "input line exceeds 16 bytes");
    }

    #[test]
    fn test_is_initialized_notification_only_matches_valid_notification() {
        assert!(is_initialized_notification(
            r#"{"jsonrpc":"2.0","method":"initialized"}"#
        ));
        assert!(is_initialized_notification(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        ));
        assert!(!is_initialized_notification(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialized"}"#
        ));
    }

    #[test]
    fn test_approval_summary_omits_message_body() {
        let tool = tools::find_tool("postblox_draft_create").unwrap();
        let summary = approval_summary(
            tool,
            &json!({
                "account_id": "a",
                "subject": "hello",
                "text_body": "private body"
            }),
        );
        assert!(summary.contains("account_id"));
        assert!(summary.contains("subject"));
        assert!(!summary.contains("private body"));
    }
}
