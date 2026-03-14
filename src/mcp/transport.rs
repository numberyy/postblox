use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::PostbloxClient;
use crate::tools;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "postblox-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn run(client: PostbloxClient) -> std::io::Result<()> {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut lines = stdin.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                write_response(&mut stdout, &err_resp).await?;
                continue;
            }
        };

        // notifications have no id — don't respond
        if request.get("id").is_none() {
            continue;
        }

        let id = request["id"].clone();
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(json!({}));

        let response = match method {
            "initialize" => handle_initialize(id, &params),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            "tools/list" => handle_tools_list(id),
            "tools/call" => handle_tools_call(id.clone(), &params, &client).await,
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            }),
        };

        write_response(&mut stdout, &response).await?;
    }

    Ok(())
}

fn handle_initialize(id: Value, _params: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
        }
    })
}

fn handle_tools_list(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "tools": tools::tool_definitions() }
    })
}

async fn handle_tools_call(id: Value, params: &Value, client: &PostbloxClient) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    match tools::dispatch(client, name, args).await {
        Ok(text) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": text }]
            }
        }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": format!("Error: {e}") }],
                "isError": true
            }
        }),
    }
}

async fn write_response(stdout: &mut tokio::io::Stdout, response: &Value) -> std::io::Result<()> {
    let mut buf = serde_json::to_vec(response)?;
    buf.push(b'\n');
    stdout.write_all(&buf).await?;
    stdout.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_initialize_returns_protocol_version() {
        let resp = handle_initialize(json!(1), &json!({}));
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn test_handle_initialize_with_client_info() {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "claude-code", "version": "1.0" }
        });
        let resp = handle_initialize(json!(42), &params);
        assert_eq!(resp["id"], 42);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn test_handle_tools_list_returns_all_tools() {
        let resp = handle_tools_list(json!(2));
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("inputSchema").is_some());
        }
    }

    #[test]
    fn test_handle_tools_list_id_preserved() {
        let resp = handle_tools_list(json!("string-id"));
        assert_eq!(resp["id"], "string-id");
    }

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool() {
        let client = PostbloxClient::new("http://localhost:1".into(), "key".into()).unwrap();
        let params = json!({ "name": "nonexistent", "arguments": {} });
        let resp = handle_tools_call(json!(3), &params, &client).await;
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown tool"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_missing_args() {
        let client = PostbloxClient::new("http://localhost:1".into(), "key".into()).unwrap();
        let params = json!({ "name": "postblox_get_message", "arguments": {} });
        let resp = handle_tools_call(json!(4), &params, &client).await;
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing required argument"));
    }

    #[tokio::test]
    async fn test_handle_tools_call_success_format() {
        // postblox_list_inboxes doesn't need args, but will fail HTTP — that's fine,
        // we're testing dispatch routing, not HTTP success
        let client = PostbloxClient::new("http://localhost:1".into(), "key".into()).unwrap();
        let params = json!({ "name": "postblox_list_inboxes", "arguments": {} });
        let resp = handle_tools_call(json!(5), &params, &client).await;
        // will be an error because localhost:1 isn't real, but it should be a tool error not a protocol error
        assert_eq!(resp["result"]["isError"], true);
        assert!(resp.get("error").is_none()); // no JSON-RPC error — tool errors go in result
    }

    #[test]
    fn test_parse_valid_jsonrpc_request() {
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let parsed: Value = serde_json::from_str(input).unwrap();
        assert_eq!(parsed["method"], "ping");
        assert_eq!(parsed["id"], 1);
    }

    #[test]
    fn test_parse_notification_has_no_id() {
        let input = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let parsed: Value = serde_json::from_str(input).unwrap();
        assert!(parsed.get("id").is_none());
    }

    #[test]
    fn test_parse_invalid_json() {
        let input = "not json at all";
        assert!(serde_json::from_str::<Value>(input).is_err());
    }
}
