use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub const JSONRPC_VERSION: &str = "2.0";
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    pub fn server(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        Self {
            code: -32000,
            message: message.into(),
            data: Some(json!({ "code": code })),
        }
    }
}

pub fn parse_line(line: &str) -> Result<Incoming, Value> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| error_response(Value::Null, JsonRpcError::parse_error(e.to_string())))?;
    parse_value(value).map_err(|(id, err)| error_response(id.unwrap_or(Value::Null), err))
}

pub fn parse_value(value: Value) -> Result<Incoming, (Option<Value>, JsonRpcError)> {
    let object = value.as_object().ok_or_else(|| {
        (
            None,
            JsonRpcError::invalid_request("message must be an object"),
        )
    })?;

    let id = object.get("id").cloned();

    match object.get("jsonrpc").and_then(Value::as_str) {
        Some(JSONRPC_VERSION) => {}
        _ => return Err((id, JsonRpcError::invalid_request("jsonrpc must be \"2.0\""))),
    }

    let method = match object.get("method").and_then(Value::as_str) {
        Some(method) if !method.is_empty() => method.to_string(),
        _ if object.contains_key("result") || object.contains_key("error") => {
            return Ok(Incoming::Response)
        }
        _ => {
            return Err((
                id,
                JsonRpcError::invalid_request("method must be a non-empty string"),
            ))
        }
    };

    if let Some(id) = id {
        if !is_valid_id(&id) {
            return Err((
                Some(id),
                JsonRpcError::invalid_request("id must be a string, number, or null"),
            ));
        }
        Ok(Incoming::Request {
            id,
            method,
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })
    } else {
        Ok(Incoming::Notification {
            method,
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })
    }
}

pub fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result,
    })
}

pub fn error_response(id: Value, error: JsonRpcError) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": error,
    })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "method": method,
        "params": params,
    })
}

pub fn initialize_result(client_protocol: Option<&str>) -> Value {
    json!({
        "protocolVersion": client_protocol.unwrap_or(MCP_PROTOCOL_VERSION),
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        },
        "serverInfo": {
            "name": "postblox-mcp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

pub fn object_params(params: Value) -> Result<Map<String, Value>, JsonRpcError> {
    match params {
        Value::Null => Ok(Map::new()),
        Value::Object(object) => Ok(object),
        _ => Err(JsonRpcError::invalid_params("params must be an object")),
    }
}

fn is_valid_id(id: &Value) -> bool {
    matches!(id, Value::String(_) | Value::Number(_) | Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_request_preserves_id_method_and_params() {
        let incoming =
            parse_line(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{"x":1}}"#)
                .unwrap();
        assert_eq!(
            incoming,
            Incoming::Request {
                id: json!(7),
                method: "tools/list".into(),
                params: json!({"x": 1}),
            }
        );
    }

    #[test]
    fn test_parse_notification_has_no_response_id() {
        let incoming = parse_line(r#"{"jsonrpc":"2.0","method":"initialized"}"#).unwrap();
        assert_eq!(
            incoming,
            Incoming::Notification {
                method: "initialized".into(),
                params: Value::Null,
            }
        );
    }

    #[test]
    fn test_malformed_json_returns_json_rpc_parse_error() {
        let err = parse_line("{").unwrap_err();
        assert_eq!(err["jsonrpc"], JSONRPC_VERSION);
        assert_eq!(err["id"], Value::Null);
        assert_eq!(err["error"]["code"], -32700);
    }

    #[test]
    fn test_invalid_request_returns_json_rpc_error_with_id_when_present() {
        let err = parse_line(r#"{"jsonrpc":"2.0","id":"abc","params":{}}"#).unwrap_err();
        assert_eq!(err["id"], "abc");
        assert_eq!(err["error"]["code"], -32600);
    }

    #[test]
    fn test_success_response_serializes_json_rpc_shape() {
        let response = success_response(json!("req-1"), json!({"ok": true}));
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains(r#""jsonrpc":"2.0""#));
        assert!(encoded.contains(r#""id":"req-1""#));
        assert!(encoded.contains(r#""result":{"ok":true}"#));
    }

    #[test]
    fn test_initialize_result_reports_server_info_and_capabilities() {
        let result = initialize_result(Some("2025-06-18"));
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(result["serverInfo"]["name"], "postblox-mcp");
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    }
}
