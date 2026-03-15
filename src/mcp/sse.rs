use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};

use crate::client::PostbloxClient;
use crate::transport;

struct SseState {
    client: PostbloxClient,
}

pub(crate) fn build_router(client: PostbloxClient) -> Router {
    let state = Arc::new(SseState { client });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    Router::new()
        .route("/jsonrpc", axum::routing::post(handle_jsonrpc))
        .route("/sse", axum::routing::get(handle_sse))
        .with_state(state)
        .layer(cors)
}

pub(crate) async fn run_sse(client: PostbloxClient, bind: &str, port: u16) -> std::io::Result<()> {
    let app = build_router(client);
    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    eprintln!("postblox-mcp SSE server listening on {bind}:{port}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    eprintln!("postblox-mcp SSE server shut down");
    Ok(())
}

async fn handle_jsonrpc(
    State(state): State<Arc<SseState>>,
    body: Bytes,
) -> axum::response::Response {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return Json(transport::parse_error(e)).into_response(),
    };

    match transport::dispatch_request(&request, &state.client).await {
        Some(response) => Json(response).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn handle_sse() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = stream::once(async { Ok(Event::default().event("endpoint").data("/jsonrpc")) });

    let pings = stream::unfold((), |_| async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Some((Ok(Event::default().event("ping").data("keepalive")), ()))
    });

    Sse::new(initial.chain(pings))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_client() -> PostbloxClient {
        PostbloxClient::new("http://localhost:1".into(), "test-key".into()).unwrap()
    }

    fn post_jsonrpc(body: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/jsonrpc")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn response_json(resp: axum::http::Response<Body>) -> Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_initialize() {
        let app = build_router(test_client());
        let req = post_jsonrpc(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(json["result"]["serverInfo"]["name"], "postblox-mcp");
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_ping() {
        let app = build_router(test_client());
        let req = post_jsonrpc(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["id"], 2);
        assert!(json["result"].is_object());
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_tools_list() {
        let app = build_router(test_client());
        let req = post_jsonrpc(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        let tools = json["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_notification_returns_204() {
        let app = build_router(test_client());
        let req = post_jsonrpc(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_invalid_json() {
        let app = build_router(test_client());
        let req = post_jsonrpc("not valid json");
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_unknown_method() {
        let app = build_router(test_client());
        let req = post_jsonrpc(r#"{"jsonrpc":"2.0","id":5,"method":"foo/bar"}"#);
        let resp = app.oneshot(req).await.unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_sse_jsonrpc_unknown_tool_actionable_error() {
        let app = build_router(test_client());
        let req = post_jsonrpc(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"bad_tool","arguments":{}}}"#,
        );
        let resp = app.oneshot(req).await.unwrap();
        let json = response_json(resp).await;
        assert_eq!(json["result"]["isError"], true);
        let text = json["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unknown tool"));
        assert!(text.contains("tools/list"));
    }

    #[tokio::test]
    async fn test_sse_endpoint_returns_event_stream() {
        let app = build_router(test_client());
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/sse")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("text/event-stream"));
    }

    #[tokio::test]
    async fn test_sse_cors_headers_on_post() {
        let app = build_router(test_client());
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/jsonrpc")
            .header("content-type", "application/json")
            .header("origin", "http://localhost:5173")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.headers().contains_key("access-control-allow-origin"));
    }

    #[tokio::test]
    async fn test_sse_cors_preflight() {
        let app = build_router(test_client());
        let req = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/jsonrpc")
            .header("origin", "http://localhost:5173")
            .header("access-control-request-method", "POST")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().contains_key("access-control-allow-origin"));
        assert!(resp.headers().contains_key("access-control-allow-methods"));
    }
}
