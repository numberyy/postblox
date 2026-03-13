use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;

use crate::api::AppState;

pub async fn ws_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let key = match extract_key_from_cookie(&headers) {
        Some(k) => k,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let stored = match crate::api::auth::validate_api_key(&state.pool, &key).await {
        Ok(k) => k,
        Err(()) => return StatusCode::UNAUTHORIZED.into_response(),
    };

    let hub = state.ws_hub.clone();
    let org_id = stored.org_id;
    ws.on_upgrade(move |socket| async move { hub.handle_ws(socket, org_id).await })
        .into_response()
}

pub(super) fn extract_key_from_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("postblox_key=") {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with_cookie(val: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(val).unwrap());
        h
    }

    #[test]
    fn test_extract_cookie_found() {
        let h = headers_with_cookie("other=val; postblox_key=pb_test1234; foo=bar");
        assert_eq!(extract_key_from_cookie(&h), Some("pb_test1234".into()));
    }

    #[test]
    fn test_extract_cookie_only_key() {
        let h = headers_with_cookie("postblox_key=pb_abc");
        assert_eq!(extract_key_from_cookie(&h), Some("pb_abc".into()));
    }

    #[test]
    fn test_extract_cookie_not_found() {
        let h = headers_with_cookie("session=xyz; theme=dark");
        assert_eq!(extract_key_from_cookie(&h), None);
    }

    #[test]
    fn test_extract_cookie_empty_value() {
        let h = headers_with_cookie("postblox_key=");
        assert_eq!(extract_key_from_cookie(&h), None);
    }

    #[test]
    fn test_extract_cookie_no_header() {
        let h = HeaderMap::new();
        assert_eq!(extract_key_from_cookie(&h), None);
    }

    #[test]
    fn test_extract_cookie_prefix_match_only() {
        let h = headers_with_cookie("postblox_key_v2=should_not_match");
        assert_eq!(extract_key_from_cookie(&h), None);
    }

    #[test]
    fn test_extract_cookie_with_spaces() {
        let h = headers_with_cookie("  postblox_key=pb_spaced  ;  other=val  ");
        assert_eq!(extract_key_from_cookie(&h), Some("pb_spaced".into()));
    }
}
