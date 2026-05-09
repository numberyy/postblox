//! On-the-wire JSON shapes.
//!
//! A frame is one of three top-level objects, distinguished by which
//! discriminator key is present:
//!
//! - request:  `{ "id": N, "op": "...", "args": {...} }`
//! - response: `{ "id": N, "ok": true|false, "data": ..., "error": {...}? }`
//! - event:    `{ "sub": N, "topic": "...", "data": ... }`
//!
//! We use `serde(untagged)` plus required-key disambiguation rather than
//! a `kind: "..."` discriminator to keep the wire compact and the
//! reference clients (TUI, MCP) easy to write.

use serde::{Deserialize, Serialize};

/// Client → daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub op: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Daemon → client (in reply to a Request with the matching `id`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Daemon → client, pushed for an active subscription.
///
/// The server encodes events from `EventOut` (`super::server::EventOut`)
/// which holds the topic as `&'static str`; the wire shape is identical.
/// Client-side `Event` keeps `String` so existing consumers can call
/// `event.topic.as_str()` without lifetime bounds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub sub: u64,
    pub topic: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Tagged error used inside [`Response`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

impl RpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unknown_op(op: &str) -> Self {
        Self::new("unknown_op", format!("unknown op '{op}'"))
    }

    pub fn bad_args(message: impl Into<String>) -> Self {
        Self::new("bad_args", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

/// Wire frame: one of the three shapes above. Matched by required keys,
/// not by an explicit discriminator, so existing clients don't need to
/// know about new frame kinds we add later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
#[non_exhaustive]
pub enum Frame {
    Request(Request),
    Response(Response),
    Event(Event),
}

impl Response {
    pub fn ok(id: u64, data: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            data,
            error: None,
        }
    }

    pub fn err(id: u64, error: RpcError) -> Self {
        Self {
            id,
            ok: false,
            data: serde_json::Value::Null,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_round_trip() {
        let r = Request {
            id: 42,
            op: "ping".into(),
            args: json!({"a": 1}),
        };
        let s = serde_json::to_string(&r).unwrap();
        let r2: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn test_request_with_missing_args_defaults_to_null() {
        let r: Request = serde_json::from_str(r#"{"id":1,"op":"ping"}"#).unwrap();
        assert!(r.args.is_null());
    }

    #[test]
    fn test_response_ok_omits_error_field() {
        let r = Response::ok(1, json!({"v": 7}));
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("error"));
        assert!(s.contains("\"ok\":true"));
    }

    #[test]
    fn test_response_err_carries_error_payload() {
        let r = Response::err(1, RpcError::unknown_op("frob"));
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("unknown_op"));
        assert!(s.contains("frob"));
    }

    #[test]
    fn test_response_round_trip_preserves_error_shape() {
        let r = Response::err(1, RpcError::bad_args("missing 'to'"));
        let s = serde_json::to_string(&r).unwrap();
        let r2: Response = serde_json::from_str(&s).unwrap();
        assert_eq!(r, r2);
    }

    #[test]
    fn test_event_round_trip() {
        let e = Event {
            sub: 9,
            topic: "mail.new".into(),
            data: json!({"id": "abc"}),
        };
        let s = serde_json::to_string(&e).unwrap();
        let e2: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(e, e2);
    }

    #[test]
    fn test_frame_disambiguates_request() {
        let f: Frame = serde_json::from_str(r#"{"id":1,"op":"ping"}"#).unwrap();
        assert!(matches!(f, Frame::Request(_)));
    }

    #[test]
    fn test_frame_disambiguates_response() {
        let f: Frame = serde_json::from_str(r#"{"id":1,"ok":true,"data":{"x":1}}"#).unwrap();
        assert!(matches!(f, Frame::Response(_)));
    }

    #[test]
    fn test_frame_disambiguates_event() {
        let f: Frame = serde_json::from_str(r#"{"sub":2,"topic":"mail.new","data":{}}"#).unwrap();
        assert!(matches!(f, Frame::Event(_)));
    }

    #[test]
    fn test_frame_serialization_round_trip_request() {
        let f = Frame::Request(Request {
            id: 1,
            op: "x".into(),
            args: json!(null),
        });
        let s = serde_json::to_string(&f).unwrap();
        let f2: Frame = serde_json::from_str(&s).unwrap();
        assert_eq!(f, f2);
    }
}
