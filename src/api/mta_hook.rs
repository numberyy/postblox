use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::AppState;

#[derive(Deserialize)]
pub struct MtaHookRequest {
    pub stage: String,
    #[serde(default, rename = "rawMessage")]
    pub raw_message: Option<String>,
}

#[derive(Serialize)]
pub struct MtaHookResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<SetOp>>,
}

#[derive(Serialize)]
pub struct SetOp {
    pub path: String,
    pub value: serde_json::Value,
}

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MtaHookRequest>,
) -> Result<Json<MtaHookResponse>, ApiError> {
    // Verify Basic auth using inbound_token as password
    if let Some(ref expected) = state.inbound_token {
        let authed = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|creds| {
                let (_, pass) = creds.split_once(':')?;
                Some(crate::api::auth::constant_time_eq(
                    pass.as_bytes(),
                    expected.as_bytes(),
                ))
            })
            .unwrap_or(false);

        if !authed {
            return Err(ApiError::Unauthorized);
        }
    }

    // Only process at the data stage (full message available)
    if payload.stage != "data" {
        return Ok(Json(MtaHookResponse { set: None }));
    }

    let raw_b64 = payload
        .raw_message
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("data stage missing rawMessage".into()))?;

    let raw_mime = base64::engine::general_purpose::STANDARD
        .decode(raw_b64)
        .map_err(|e| ApiError::BadRequest(format!("invalid base64 in rawMessage: {e}")))?;

    super::inbound::process_inbound_raw(&state, &raw_mime).await?;

    // Tell Stalwart to discard — postblox is the authoritative store
    Ok(Json(MtaHookResponse {
        set: Some(vec![SetOp {
            path: "/action".into(),
            value: serde_json::json!("discard"),
        }]),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mta_hook_response_accept_serializes_clean() {
        let resp = MtaHookResponse { set: None };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn test_mta_hook_response_discard_serializes() {
        let resp = MtaHookResponse {
            set: Some(vec![SetOp {
                path: "/action".into(),
                value: serde_json::json!("discard"),
            }]),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("/action"));
        assert!(json.contains("discard"));
    }

    #[test]
    fn test_mta_hook_request_deserialize_data_stage() {
        let json = r#"{"stage":"data","rawMessage":"dGVzdA=="}"#;
        let req: MtaHookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.stage, "data");
        assert_eq!(req.raw_message.as_deref(), Some("dGVzdA=="));
    }

    #[test]
    fn test_mta_hook_request_deserialize_connect_stage() {
        let json = r#"{"stage":"connect"}"#;
        let req: MtaHookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.stage, "connect");
        assert!(req.raw_message.is_none());
    }

    #[test]
    fn test_mta_hook_request_ignores_extra_fields() {
        let json = r#"{"stage":"ehlo","envelope":{"from":"test@example.com"},"server":{}}"#;
        let req: MtaHookRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.stage, "ehlo");
    }
}
