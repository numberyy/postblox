mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::{create_message_input, setup_inbox, setup_org, test_app, test_pool, test_state};

// -- helpers --

async fn get_json(app: &axum::Router, path: &str, key: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::get(path)
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "response not valid JSON: {e}\nbody: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

async fn post_json(
    app: &axum::Router,
    path: &str,
    key: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::post(path)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "response not valid JSON: {e}\nbody: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

async fn delete_req(app: &axum::Router, path: &str, key: &str) -> StatusCode {
    let req = Request::delete(path)
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    resp.status()
}

fn test_state_with_inbound(pool: sqlx::PgPool) -> postblox::api::AppState {
    let mut state = test_state(pool);
    state.inbound_token = Some("test-inbound-token".into());
    state
}

/// Creates org + API key + admin member record so AdminOrg extractor passes.
async fn setup_admin_org(pool: &sqlx::PgPool) -> (Uuid, String) {
    let org = postblox::db::organizations::create(pool, "Admin Org")
        .await
        .unwrap();
    let raw_key = format!("pb_{}", Uuid::new_v4().to_string().replace('-', ""));
    let hash = postblox::api::auth::hash_key(&raw_key);
    let prefix = &raw_key[..8];
    let api_key = postblox::db::api_keys::create(pool, org.id, &hash, prefix, Some("admin"))
        .await
        .unwrap();
    postblox::db::members::create(pool, org.id, api_key.id, postblox::models::Role::Admin)
        .await
        .unwrap();
    (org.id, raw_key)
}

fn raw_mime(to: &str, subject: &str, message_id: Option<&str>) -> String {
    let mut headers = format!(
        "From: sender@example.com\r\n\
         To: {to}\r\n\
         Subject: {subject}\r\n"
    );
    if let Some(mid) = message_id {
        headers.push_str(&format!("Message-ID: <{mid}>\r\n"));
    }
    headers.push_str(
        "Content-Type: text/plain\r\n\
         \r\n\
         Hello, this is a test email body.\r\n",
    );
    headers
}

// ============================================================================
// 1. Inbound Pipeline (POST /internal/stalwart/inbound)
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_valid_email_creates_message() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mime = raw_mime(
        &inbox.email,
        "Inbound test",
        Some("inbound-test-1@example.com"),
    );
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].subject.as_deref(), Some("Inbound test"));
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_missing_auth_returns_401() {
    let pool = test_pool().await;
    let state = test_state_with_inbound(pool);
    let app = test_app(state);

    let req = Request::post("/internal/stalwart/inbound")
        .body(Body::from("irrelevant"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_wrong_token_returns_401() {
    let pool = test_pool().await;
    let state = test_state_with_inbound(pool);
    let app = test_app(state);

    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer wrong-token")
        .body(Body::from("irrelevant"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_dedup_rejects_duplicate_message_id() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mime = raw_mime(&inbox.email, "Dedup test", Some("dedup-123@example.com"));

    // First send
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime.clone()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Second send with same Message-ID
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1, "duplicate should not create second message");
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_no_message_id_gets_synthetic() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mime = raw_mime(&inbox.email, "No MID", None);
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    let mid = msgs[0].message_id_header.as_deref().unwrap();
    assert!(
        mid.starts_with("synth-"),
        "expected synthetic ID, got: {mid}"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_cc_recipient_matches_inbox() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    // Inbox email is in CC, not To
    let mime = format!(
        "From: sender@example.com\r\n\
         To: someone-else@example.com\r\n\
         Cc: {}\r\n\
         Subject: CC match test\r\n\
         Message-ID: <cc-match-1@example.com>\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         CC body.\r\n",
        inbox.email
    );

    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1, "CC recipient should match inbox");
}

// ============================================================================
// 2. Messages Send / List / Get
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_message_creates_outbound() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Approval mode (default) — message stored + approval created, no delivery attempted
    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["recipient@example.com"],
        "subject": "Outbound test",
        "text_body": "Hello from test"
    });
    let (status, _resp_json) = post_json(&app, &path, &key, &body).await;

    // Default send mode is Approval → 202 Accepted (no Stalwart needed)
    assert_eq!(status, StatusCode::ACCEPTED, "expected 202, got {status}");

    // Verify message was stored
    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].subject.as_deref(), Some("Outbound test"));
    assert_eq!(msgs[0].direction, postblox::models::Direction::Outbound);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_message_empty_to_returns_400() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": [],
        "subject": "No recipients"
    });
    let (status, _) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_message_wrong_inbox_returns_404() {
    let pool = test_pool().await;
    let (_org_a, _key_a) = setup_org(&pool).await;
    let (org_b, key_b) = setup_org(&pool).await;
    let inbox_b = setup_inbox(&pool, org_b).await;

    // Create a separate org's inbox and try to use org B's key on a random inbox id
    let app = test_app(test_state(pool.clone()));
    let fake_inbox = Uuid::new_v4();
    let path = format!("/api/v1/inboxes/{fake_inbox}/messages");
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Wrong inbox"
    });
    let (status, _) = post_json(&app, &path, &key_b, &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Also verify: inbox from org B is not accessible with org A's key
    let (_org_a2, key_a2) = setup_org(&pool).await;
    let app2 = test_app(test_state(pool.clone()));
    let path2 = format!("/api/v1/inboxes/{}/messages", inbox_b.id);
    let body2 = json!({
        "to": ["test@example.com"],
        "subject": "Cross org"
    });
    let (status2, _) = post_json(&app2, &path2, &key_a2, &body2).await;
    assert_eq!(status2, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_message_stores_html_body() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Use approval mode (default) so message is stored without delivery attempt
    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "HTML test",
        "html_body": "<h1>Hello</h1>"
    });
    let (status, resp) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "approval mode → 202");

    let msg_id: Uuid = resp["id"].as_str().unwrap().parse().unwrap();
    let msg = postblox::db::messages::get_by_id(&pool, msg_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(msg.html_body.as_deref(), Some("<h1>Hello</h1>"));
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_list_messages_pagination() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    for i in 0..3 {
        postblox::db::messages::create(
            &pool,
            &create_message_input(inbox.id, &format!("Msg {i}"), &format!("Body {i}")),
        )
        .await
        .unwrap();
    }

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages?limit=2&offset=0", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_list_messages_thread_filter() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let thread = postblox::db::threads::create(&pool, inbox.id, Some("Thread A"))
        .await
        .unwrap();

    let mut cm1 = create_message_input(inbox.id, "In thread", "Body A");
    cm1.thread_id = Some(thread.id);
    postblox::db::messages::create(&pool, &cm1).await.unwrap();

    // Message not in the thread
    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Not in thread", "Body B"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!(
        "/api/v1/inboxes/{}/messages?thread_id={}",
        inbox.id, thread.id
    );
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    let msgs = json.as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["subject"], "In thread");
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_get_message_by_id() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Get me", "Body here"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages/{}", inbox.id, msg.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["id"], msg.id.to_string());
    assert_eq!(json["subject"], "Get me");
    assert_eq!(json["text_body"], "Body here");
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_get_message_wrong_org_returns_404() {
    let pool = test_pool().await;
    let (org_a, _key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;
    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_a.id, "Org A msg", "Secret"),
    )
    .await
    .unwrap();

    let (_org_b, key_b) = setup_org(&pool).await;

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages/{}", inbox_a.id, msg.id);
    let (status, _) = get_json(&app, &path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// 3. Drafts
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_create_draft() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let body = json!({
        "to": ["draft-rcpt@example.com"],
        "subject": "Draft subject",
        "text_body": "Draft body"
    });
    let (status, json) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["subject"], "Draft subject");
    assert!(json["id"].as_str().is_some());
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_list_drafts() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    for i in 0..2 {
        let path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
        let body = json!({
            "subject": format!("Draft {i}"),
            "text_body": format!("Body {i}")
        });
        let (status, _) = post_json(&app, &path, &key, &body).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_delete_draft() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    let create_path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let body = json!({ "subject": "To be deleted" });
    let (status, json) = post_json(&app, &create_path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = json["id"].as_str().unwrap();

    let delete_path = format!("/api/v1/inboxes/{}/drafts/{}", inbox.id, draft_id);
    let status = delete_req(&app, &delete_path, &key).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify gone
    let draft_uuid: Uuid = draft_id.parse().unwrap();
    let fetched = postblox::db::drafts::get_by_id(&pool, draft_uuid)
        .await
        .unwrap();
    assert!(fetched.is_none());
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_delete_draft_wrong_org_returns_404() {
    let pool = test_pool().await;
    let (org_a, key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;
    let app = test_app(test_state(pool.clone()));

    let path = format!("/api/v1/inboxes/{}/drafts", inbox_a.id);
    let body = json!({ "subject": "Org A draft" });
    let (status, json) = post_json(&app, &path, &key_a, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = json["id"].as_str().unwrap();

    let (_org_b, key_b) = setup_org(&pool).await;
    let delete_path = format!("/api/v1/inboxes/{}/drafts/{}", inbox_a.id, draft_id);
    let status = delete_req(&app, &delete_path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// 4. Inboxes
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_create_inbox() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let body = json!({
        "email": format!("new-inbox-{}@test.example.com", Uuid::new_v4()),
        "display_name": "My New Inbox"
    });
    let (status, json) = post_json(&app, "/api/v1/inboxes", &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["display_name"], "My New Inbox");
    assert!(json["id"].as_str().is_some());
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_list_inboxes() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    setup_inbox(&pool, org_id).await;
    setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let (status, json) = get_json(&app, "/api/v1/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_get_inbox() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["id"], inbox.id.to_string());
    assert_eq!(json["email"], inbox.email);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_delete_inbox() {
    let pool = test_pool().await;
    let (org_id, admin_key) = setup_admin_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    let path = format!("/api/v1/inboxes/{}", inbox.id);
    let status = delete_req(&app, &path, &admin_key).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let fetched = postblox::db::inboxes::get_by_id(&pool, inbox.id)
        .await
        .unwrap();
    assert!(fetched.is_none());
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_cross_org_isolation() {
    let pool = test_pool().await;
    let (org_a, key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;

    let (_org_b, key_b) = setup_org(&pool).await;

    let app = test_app(test_state(pool));

    // Org B cannot see Org A's inbox
    let path = format!("/api/v1/inboxes/{}", inbox_a.id);
    let (status, _) = get_json(&app, &path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Org B lists inboxes and gets empty
    let (status, json) = get_json(&app, "/api/v1/inboxes", &key_b).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.as_array().unwrap().is_empty());

    // Org A sees its own inbox
    let (status, json) = get_json(&app, "/api/v1/inboxes", &key_a).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["id"] == inbox_a.id.to_string()));
}

// ============================================================================
// 5. Cross-entity authorization
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_message_in_wrong_inbox_returns_404() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_id).await;
    let inbox_b = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_a.id, "Inbox A msg", "Body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));

    // Message belongs to inbox_a, request via inbox_b path
    let path = format!("/api/v1/inboxes/{}/messages/{}", inbox_b.id, msg.id);
    let (status, _) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_attachment_cross_org_isolation() {
    let pool = test_pool().await;
    let (org_a, _key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_a.id, "With attachment", "Body"),
    )
    .await
    .unwrap();

    let att = postblox::db::attachments::create(
        &pool,
        &postblox::models::CreateAttachment {
            message_id: msg.id,
            filename: "test.txt".into(),
            content_type: "text/plain".into(),
            size_bytes: 5,
            storage_key: "fake-key".into(),
            disposition: postblox::models::Disposition::Attachment,
            content_id: None,
        },
    )
    .await
    .unwrap();

    let (_org_b, key_b) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    // Org B tries to list attachments on Org A's message
    let path = format!(
        "/api/v1/inboxes/{}/messages/{}/attachments",
        inbox_a.id, msg.id
    );
    let (status, _) = get_json(&app, &path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Org B tries to download specific attachment
    let path = format!(
        "/api/v1/inboxes/{}/messages/{}/attachments/{}",
        inbox_a.id, msg.id, att.id
    );
    let (status, _) = get_json(&app, &path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ============================================================================
// 6. Search
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_empty_query_returns_400() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = get_json(&app, "/api/v1/search?q=", &key).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_whitespace_only_returns_400() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = get_json(&app, "/api/v1/search?q=%20%20%20", &key).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_text_returns_matching_messages() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Unique alpha subject", "body"),
    )
    .await
    .unwrap();
    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Unrelated", "nothing here"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, json) = get_json(&app, "/api/v1/search?q=alpha", &key).await;
    assert_eq!(status, StatusCode::OK);
    let results = json.as_array().unwrap();
    assert!(
        results.iter().all(|m| {
            let subject = m["subject"].as_str().unwrap_or("");
            let body = m["text_body"].as_str().unwrap_or("");
            subject.contains("alpha") || body.contains("alpha")
        }),
        "all results should match query"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_field_from() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let mut cm = create_message_input(inbox.id, "Field search", "body");
    cm.from_addr = "specialsender@example.com".into();
    postblox::db::messages::create(&pool, &cm).await.unwrap();

    postblox::db::messages::create(&pool, &create_message_input(inbox.id, "Other msg", "body"))
        .await
        .unwrap();

    let app = test_app(test_state(pool));
    let (status, json) = get_json(
        &app,
        "/api/v1/search?q=@from:specialsender@example.com",
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = json.as_array().unwrap();
    assert!(
        !results.is_empty(),
        "field search should find the message from specialsender"
    );
    assert!(results
        .iter()
        .all(|m| m["from_addr"] == "specialsender@example.com"));
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_semantic_without_provider_returns_400() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = get_json(&app, "/api/v1/search?q=hello&semantic=true", &key).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_search_cross_org_isolation() {
    let pool = test_pool().await;
    let (org_a, _key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_a.id, "Secret org A data", "confidential"),
    )
    .await
    .unwrap();

    let (_org_b, key_b) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, json) = get_json(&app, "/api/v1/search?q=confidential", &key_b).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json.as_array().unwrap().is_empty(),
        "org B should not see org A's messages"
    );
}

// ============================================================================
// 7. Labels
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_create_label() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}/labels", inbox.id);
    let body = json!({"name": "Important", "color": "#ff0000"});
    let (status, json) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["name"], "Important");
    assert_eq!(json["color"], "#ff0000");
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_add_label_to_message_and_list() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let label = postblox::db::labels::create(&pool, inbox.id, "urgent", Some("#ff0000"))
        .await
        .unwrap();
    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Labeled msg", "body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let add_path = format!("/api/v1/inboxes/{}/messages/{}/labels", inbox.id, msg.id);
    let (status, _) = post_json(&app, &add_path, &key, &json!({"label_id": label.id})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // List labels for message
    let (status, json) = get_json(&app, &add_path, &key).await;
    assert_eq!(status, StatusCode::OK);
    let labels = json.as_array().unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0]["name"], "urgent");
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_remove_label_from_message() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let label = postblox::db::labels::create(&pool, inbox.id, "temp", None)
        .await
        .unwrap();
    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Remove label test", "body"),
    )
    .await
    .unwrap();
    postblox::db::labels::add_to_message(&pool, msg.id, label.id)
        .await
        .unwrap();

    let app = test_app(test_state(pool));
    let remove_path = format!(
        "/api/v1/inboxes/{}/messages/{}/labels/{}",
        inbox.id, msg.id, label.id
    );
    let status = delete_req(&app, &remove_path, &key).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify label was removed
    let list_path = format!("/api/v1/inboxes/{}/messages/{}/labels", inbox.id, msg.id);
    let (status, json) = get_json(&app, &list_path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_label_cross_inbox_isolation() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_id).await;
    let inbox_b = setup_inbox(&pool, org_id).await;

    // Create label in inbox A
    let label_a = postblox::db::labels::create(&pool, inbox_a.id, "shared-name", None)
        .await
        .unwrap();

    // Create message in inbox B
    let msg_b = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_b.id, "Inbox B msg", "body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));

    // Try to add inbox A's label to inbox B's message — should fail
    let path = format!(
        "/api/v1/inboxes/{}/messages/{}/labels",
        inbox_b.id, msg_b.id
    );
    let (status, _) = post_json(&app, &path, &key, &json!({"label_id": label_a.id})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_create_label_empty_name_returns_400() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}/labels", inbox.id);
    let body = json!({"name": "  "});
    let (status, _) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ============================================================================
// 8. Multi-tenancy Isolation
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_multi_tenancy_full_isolation() {
    let pool = test_pool().await;

    // Set up two completely independent orgs
    let (org_a, key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;
    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_a.id, "Org A secret message", "classified A"),
    )
    .await
    .unwrap();

    let (org_b, key_b) = setup_org(&pool).await;
    let inbox_b = setup_inbox(&pool, org_b).await;
    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox_b.id, "Org B secret message", "classified B"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));

    // Org A cannot see Org B's inboxes
    let (status, json) = get_json(&app, "/api/v1/inboxes", &key_a).await;
    assert_eq!(status, StatusCode::OK);
    let inbox_ids: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(
        !inbox_ids.contains(&inbox_b.id.to_string().as_str()),
        "org A should not see org B's inbox"
    );

    // Org B cannot see Org A's inboxes
    let (status, json) = get_json(&app, "/api/v1/inboxes", &key_b).await;
    assert_eq!(status, StatusCode::OK);
    let inbox_ids: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(
        !inbox_ids.contains(&inbox_a.id.to_string().as_str()),
        "org B should not see org A's inbox"
    );

    // Org A cannot access Org B's inbox directly
    let path = format!("/api/v1/inboxes/{}", inbox_b.id);
    let (status, _) = get_json(&app, &path, &key_a).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Org A cannot list Org B's messages
    let path = format!("/api/v1/inboxes/{}/messages", inbox_b.id);
    let (status, _) = get_json(&app, &path, &key_a).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Org B cannot list Org A's messages
    let path = format!("/api/v1/inboxes/{}/messages", inbox_a.id);
    let (status, _) = get_json(&app, &path, &key_b).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Search is org-scoped — Org A's search does not return Org B's messages
    let (status, json) = get_json(&app, "/api/v1/search?q=classified", &key_a).await;
    assert_eq!(status, StatusCode::OK);
    for msg in json.as_array().unwrap() {
        assert_ne!(
            msg["text_body"].as_str().unwrap_or(""),
            "classified B",
            "org A should not see org B's messages in search"
        );
    }
}

// ============================================================================
// 9. Audit Log Integration
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_audit_inbox_created_on_create() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool.clone()));

    let email = format!("audit-test-{}@test.example.com", Uuid::new_v4());
    let body = json!({"email": email});
    let (status, resp) = post_json(&app, "/api/v1/inboxes", &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let org_id: Uuid = resp["org_id"].as_str().unwrap().parse().unwrap();

    // Give the spawned audit task a moment to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let entries = postblox::db::audit::list_entries(
        &pool,
        org_id,
        0,
        10,
        None,
        Some("inbox_created"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        !entries.is_empty(),
        "inbox creation should produce an audit entry"
    );
    assert_eq!(
        entries[0].action,
        postblox::models::AuditAction::InboxCreated
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_audit_inbox_deleted_on_delete() {
    let pool = test_pool().await;
    let (org_id, admin_key) = setup_admin_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    let path = format!("/api/v1/inboxes/{}", inbox.id);
    let status = delete_req(&app, &path, &admin_key).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let entries = postblox::db::audit::list_entries(
        &pool,
        org_id,
        0,
        10,
        None,
        Some("inbox_deleted"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        !entries.is_empty(),
        "inbox deletion should produce an audit entry"
    );
    assert_eq!(
        entries[0].action,
        postblox::models::AuditAction::InboxDeleted
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_audit_message_received_on_inbound() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mime = raw_mime(
        &inbox.email,
        "Audit inbound test",
        Some("audit-inbound-1@example.com"),
    );
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let entries = postblox::db::audit::list_entries(
        &pool,
        org_id,
        0,
        10,
        None,
        Some("message_received"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        !entries.is_empty(),
        "inbound message should produce a message_received audit entry"
    );
    assert_eq!(
        entries[0].action,
        postblox::models::AuditAction::MessageReceived
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_audit_webhook_created_on_create() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool.clone()));

    let body = json!({
        "url": "https://hooks.example.com/webhook",
        "events": ["message.received"]
    });
    let (status, _) = post_json(&app, "/api/v1/webhooks", &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let entries = postblox::db::audit::list_entries(
        &pool,
        org_id,
        0,
        10,
        None,
        Some("webhook_created"),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        !entries.is_empty(),
        "webhook creation should produce an audit entry"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_audit_log_respects_org_boundary() {
    let pool = test_pool().await;
    let (org_a, key_a) = setup_org(&pool).await;
    let app = test_app(test_state(pool.clone()));

    // Create inbox for org A (triggers InboxCreated audit)
    let email = format!("audit-boundary-{}@test.example.com", Uuid::new_v4());
    let body = json!({"email": email});
    let (status, _) = post_json(&app, "/api/v1/inboxes", &key_a, &body).await;
    assert_eq!(status, StatusCode::CREATED);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Org B should see no audit entries
    let (org_b, _) = setup_org(&pool).await;
    let entries = postblox::db::audit::list_entries(&pool, org_b, 0, 100, None, None, None, None)
        .await
        .unwrap();
    let org_a_leaks: Vec<_> = entries.iter().filter(|e| e.org_id == org_a).collect();
    assert!(
        org_a_leaks.is_empty(),
        "org B should never see org A's audit entries"
    );
}

// ============================================================================
// 10. Inbound Pipeline — Additional Coverage
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_malformed_mime_returns_422() {
    let pool = test_pool().await;
    let state = test_state_with_inbound(pool);
    let app = test_app(state);

    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from("This is not valid MIME at all \x00\x01\x02"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "malformed MIME should return 422, not panic"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_email_with_attachment_stores_metadata() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let mut state = test_state_with_inbound(pool.clone());
    let att_dir = std::env::temp_dir().join(format!("postblox-test-att-{}", Uuid::new_v4()));
    state.attachment_storage_path = att_dir;
    let app = test_app(state);

    let boundary = "----=_Part_123_456";
    let mime = format!(
        "From: sender@example.com\r\n\
         To: {}\r\n\
         Subject: Attachment test\r\n\
         Message-ID: <att-test-1@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\
         \r\n\
         --{boundary}\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         Body text here.\r\n\
         --{boundary}\r\n\
         Content-Type: text/plain; name=\"test.txt\"\r\n\
         Content-Disposition: attachment; filename=\"test.txt\"\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n\
         SGVsbG8gV29ybGQ=\r\n\
         --{boundary}--\r\n",
        inbox.email,
    );

    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);

    let attachments = postblox::db::attachments::list_by_message(&pool, msgs[0].id)
        .await
        .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "test.txt");
    assert!(attachments[0].size_bytes > 0);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_slop_classification_sets_score() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mime = raw_mime(&inbox.email, "Slop test", Some("slop-test-1@example.com"));
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Give slop update a moment (runs in-band, not spawned, so should be immediate)
    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    // slop_score should be set (even if 0 for a clean message)
    assert!(
        msgs[0].slop_score.is_some(),
        "inbound message should have slop_score set"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_reply_threads_to_existing() {
    let pool = test_pool().await;
    let (org_id, _) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    // First message — creates a thread
    let original_mid = "original-thread-1@example.com";
    let mime1 = raw_mime(&inbox.email, "Thread start", Some(original_mid));
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime1))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    let original_thread_id = msgs[0].thread_id.unwrap();

    // Reply with In-Reply-To referencing the original
    let reply_mime = format!(
        "From: other@example.com\r\n\
         To: {}\r\n\
         Subject: Re: Thread start\r\n\
         Message-ID: <reply-to-thread-1@example.com>\r\n\
         In-Reply-To: <{original_mid}>\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         This is a reply.\r\n",
        inbox.email,
    );
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(reply_mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 2);

    let reply_msg = msgs
        .iter()
        .find(|m| m.message_id_header.as_deref() == Some("<reply-to-thread-1@example.com>"))
        .expect("reply message should exist");
    assert_eq!(
        reply_msg.thread_id,
        Some(original_thread_id),
        "reply should be threaded to the same thread as the original"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_inbound_unknown_recipient_returns_404() {
    let pool = test_pool().await;
    let state = test_state_with_inbound(pool);
    let app = test_app(state);

    let mime = raw_mime(
        "nobody@nonexistent-domain.example.com",
        "Unknown recipient",
        Some("unknown-1@example.com"),
    );
    let req = Request::post("/internal/stalwart/inbound")
        .header("authorization", "Bearer test-inbound-token")
        .body(Body::from(mime))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "email to unknown recipient should return 404"
    );
}

// ============================================================================
// 11. Send Validation — Permission Modes & Guard
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_shadow_mode_returns_403() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Set inbox to shadow mode
    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::Shadow,
        &serde_json::json!([]),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Shadow test",
        "text_body": "Should be blocked"
    });
    let (status, resp) = post_json(&app, &path, &key, &body).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "shadow mode should block sending: {resp}"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_approval_mode_returns_202_with_approval() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Explicitly set Approval mode
    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::Approval,
        &serde_json::json!([]),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Approval test",
        "text_body": "Needs approval"
    });
    let (status, resp) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "approval mode → 202");

    let msg_id: Uuid = resp["id"].as_str().unwrap().parse().unwrap();

    // Give spawned tasks a moment
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify an approval was created
    let approvals = postblox::db::approvals::list_by_status(&pool, org_id, None, 0, 10)
        .await
        .unwrap();
    assert!(
        approvals.iter().any(|a| a.message_id == msg_id),
        "approval should be created for the message"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_auto_approve_failing_rules_returns_403() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // AutoApprove with a domain allowlist rule that won't match the recipient
    let rules = serde_json::json!([
        {
            "type": "domain_allowlist",
            "domains": ["allowed.example.com"]
        }
    ]);
    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::AutoApprove,
        &rules,
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@blocked-domain.com"],
        "subject": "Rule fail test",
        "text_body": "Should be blocked by rule"
    });
    let (status, _) = post_json(&app, &path, &key, &body).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "auto_approve with failing rule → 403"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_auto_approve_passing_rules_attempts_delivery() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // AutoApprove with a domain allowlist that matches recipient
    let rules = serde_json::json!([
        {
            "type": "domain_allowlist",
            "domains": ["example.com"]
        }
    ]);
    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::AutoApprove,
        &rules,
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "AutoApprove pass test",
        "text_body": "Should attempt delivery"
    });
    let (status, _) = post_json(&app, &path, &key, &body).await;

    // No Stalwart configured → delivery fails with 500, but that proves
    // no approval was created (it went past the approval gate)
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "auto_approve with passing rules should attempt delivery (fails without Stalwart)"
    );

    // Verify NO approval was created
    let approvals = postblox::db::approvals::list_by_status(&pool, org_id, None, 0, 10)
        .await
        .unwrap();
    assert!(
        approvals.is_empty(),
        "auto_approve should not create an approval"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_guard_violation_blocks_message() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let mut state = test_state(pool);
    state.guard_patterns = postblox::mail::guard::default_patterns();
    let app = test_app(state);

    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "My AWS key",
        "text_body": "Here is my key: AKIAIOSFODNN7EXAMPLE"
    });
    let (status, resp) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err_msg = resp["error"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("blocked"),
        "guard violation should mention 'blocked', got: {err_msg}"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_cross_org_isolation_forbidden() {
    let pool = test_pool().await;
    let (org_a, _key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;

    let (_org_b, key_b) = setup_org(&pool).await;

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox_a.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Cross-org send"
    });
    let (status, _) = post_json(&app, &path, &key_b, &body).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "org B should not be able to send from org A's inbox"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_before_send_hook_blocks_message() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let hooks = vec![postblox::hooks::HookConfig {
        event: "before_send".into(),
        command: "bash".into(),
        args: vec![
            "-c".into(),
            r#"cat > /dev/null; echo '{"action":"block","reason":"hook says no"}'"#.into(),
        ],
        timeout_secs: 5,
    }];
    let mut state = test_state(pool);
    state.hooks = Arc::from(hooks);
    let app = test_app(state);

    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Hook block test",
        "text_body": "Should be blocked by hook"
    });
    let (status, resp) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let err_msg = resp["error"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("hook says no"),
        "hook block reason should appear in error: {err_msg}"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_no_auth_returns_401() {
    let pool = test_pool().await;
    let (org_id, _key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let req = Request::post(&path)
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&json!({
                "to": ["test@example.com"],
                "subject": "No auth"
            }))
            .unwrap(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// 12. Drafts — Send & Guard
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_draft_creates_message_and_deletes_draft() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Default mode is Approval (no permission row = approval)
    let app = test_app(test_state(pool.clone()));

    // Create draft
    let draft_path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let body = json!({
        "to": ["draft-send-rcpt@example.com"],
        "subject": "Send draft test",
        "text_body": "Draft content"
    });
    let (status, draft_json) = post_json(&app, &draft_path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id: Uuid = draft_json["id"].as_str().unwrap().parse().unwrap();

    // Send the draft
    let send_path = format!("/api/v1/inboxes/{}/drafts/{}/send", inbox.id, draft_id);
    let req = Request::post(&send_path)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();

    // Approval mode → 202
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "draft send in approval mode → 202"
    );

    // Draft should be deleted
    let fetched = postblox::db::drafts::get_by_id(&pool, draft_id)
        .await
        .unwrap();
    assert!(fetched.is_none(), "draft should be deleted after sending");

    // Message should exist
    let msgs = postblox::db::messages::list_by_inbox(&pool, inbox.id, 10, 0)
        .await
        .unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].subject.as_deref(), Some("Send draft test"));
    assert_eq!(msgs[0].direction, postblox::models::Direction::Outbound);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_draft_guard_violation_preserves_draft() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let mut state = test_state(pool.clone());
    state.guard_patterns = postblox::mail::guard::default_patterns();
    let app = test_app(state);

    // Create a draft containing a secret
    let draft_path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let body = json!({
        "to": ["rcpt@example.com"],
        "subject": "Secret draft",
        "text_body": "Key: AKIAIOSFODNN7EXAMPLE"
    });
    let (status, draft_json) = post_json(&app, &draft_path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id: Uuid = draft_json["id"].as_str().unwrap().parse().unwrap();

    // Try to send — guard should block
    let send_path = format!("/api/v1/inboxes/{}/drafts/{}/send", inbox.id, draft_id);
    let req = Request::post(&send_path)
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Draft should still exist
    let fetched = postblox::db::drafts::get_by_id(&pool, draft_id)
        .await
        .unwrap();
    assert!(
        fetched.is_some(),
        "draft should be preserved when guard blocks send"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_draft_empty_to_returns_400() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    // Create a draft with no recipients
    let draft_path = format!("/api/v1/inboxes/{}/drafts", inbox.id);
    let body = json!({
        "subject": "No recipients draft"
    });
    let (status, draft_json) = post_json(&app, &draft_path, &key, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = draft_json["id"].as_str().unwrap();

    // Try to send
    let send_path = format!("/api/v1/inboxes/{}/drafts/{}/send", inbox.id, draft_id);
    let req = Request::post(&send_path)
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "sending draft with no recipients → 400"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_send_draft_cross_org_returns_404() {
    let pool = test_pool().await;
    let (org_a, key_a) = setup_org(&pool).await;
    let inbox_a = setup_inbox(&pool, org_a).await;
    let app = test_app(test_state(pool.clone()));

    // Org A creates a draft
    let draft_path = format!("/api/v1/inboxes/{}/drafts", inbox_a.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Org A draft"
    });
    let (status, draft_json) = post_json(&app, &draft_path, &key_a, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let draft_id = draft_json["id"].as_str().unwrap();

    // Org B tries to send it
    let (_org_b, key_b) = setup_org(&pool).await;
    let send_path = format!("/api/v1/inboxes/{}/drafts/{}/send", inbox_a.id, draft_id);
    let req = Request::post(&send_path)
        .header("authorization", format!("Bearer {key_b}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "org B should not be able to send org A's draft"
    );
}

// ============================================================================
// 13. Rate Limiting — HTTP-level Integration
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_rate_limit_returns_429_after_limit() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;

    let mut state = test_state(pool);
    // Very low limit so we can trigger it quickly
    state.rate_limiter = Arc::new(postblox::api::rate_limit::RateLimiter::new(3, 100));
    let app = test_app(state);

    // First 3 requests should succeed
    for i in 0..3 {
        let (status, _) = get_json(&app, "/api/v1/inboxes", &key).await;
        assert_eq!(status, StatusCode::OK, "request {i} should succeed");
    }

    // 4th request should be rate-limited
    let req = Request::get("/api/v1/inboxes")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_rate_limit_headers_present() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;

    let mut state = test_state(pool);
    state.rate_limiter = Arc::new(postblox::api::rate_limit::RateLimiter::new(100, 10000));
    let app = test_app(state);

    let req = Request::get("/api/v1/inboxes")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(
        resp.headers().contains_key("x-ratelimit-limit"),
        "response should contain X-RateLimit-Limit header"
    );
    assert!(
        resp.headers().contains_key("x-ratelimit-remaining"),
        "response should contain X-RateLimit-Remaining header"
    );
    assert!(
        resp.headers().contains_key("x-ratelimit-reset"),
        "response should contain X-RateLimit-Reset header"
    );

    let remaining: u64 = resp
        .headers()
        .get("x-ratelimit-remaining")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        remaining, 99,
        "remaining should be limit - 1 after first request"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_rate_limit_different_keys_independent() {
    let pool = test_pool().await;
    let (_org_a, key_a) = setup_org(&pool).await;
    let (_org_b, key_b) = setup_org(&pool).await;

    let mut state = test_state(pool);
    state.rate_limiter = Arc::new(postblox::api::rate_limit::RateLimiter::new(2, 100));
    let app = test_app(state);

    // Key A uses all its quota
    for _ in 0..2 {
        let (status, _) = get_json(&app, "/api/v1/inboxes", &key_a).await;
        assert_eq!(status, StatusCode::OK);
    }
    let req = Request::get("/api/v1/inboxes")
        .header("authorization", format!("Bearer {key_a}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "key A should be limited"
    );

    // Key B should still work
    let (status, _) = get_json(&app, "/api/v1/inboxes", &key_b).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "key B should not be affected by key A's limit"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_rate_limit_unauthenticated_bypasses() {
    let pool = test_pool().await;

    let mut state = test_state(pool);
    // Even with a very low limit, unauthenticated should bypass
    state.rate_limiter = Arc::new(postblox::api::rate_limit::RateLimiter::new(1, 1));
    let app = test_app(state);

    // Multiple unauthenticated requests to /health (no auth needed)
    for _ in 0..5 {
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "unauthenticated requests should bypass rate limiting"
        );
    }
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_rate_limit_429_includes_retry_after() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;

    let mut state = test_state(pool);
    state.rate_limiter = Arc::new(postblox::api::rate_limit::RateLimiter::new(1, 100));
    let app = test_app(state);

    // Use up the limit
    let (status, _) = get_json(&app, "/api/v1/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);

    // Next request should be rate-limited with Retry-After header
    let req = Request::get("/api/v1/inboxes")
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().contains_key("retry-after"),
        "429 response should include Retry-After header"
    );
    let retry_after: u64 = resp
        .headers()
        .get("retry-after")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry_after >= 1, "Retry-After should be at least 1 second");
}

// ============================================================================
// Inbox Delete: admin-only verification
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_delete_inbox_requires_admin_role() {
    let pool = test_pool().await;
    let (org_id, non_admin_key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}", inbox.id);
    let status = delete_req(&app, &path, &non_admin_key).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin key should not be able to delete inbox"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_delete_inbox_admin_succeeds() {
    let pool = test_pool().await;
    let (org_id, admin_key) = setup_admin_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    let path = format!("/api/v1/inboxes/{}", inbox.id);
    let status = delete_req(&app, &path, &admin_key).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let fetched = postblox::db::inboxes::get_by_id(&pool, inbox.id)
        .await
        .unwrap();
    assert!(fetched.is_none(), "inbox should be deleted from DB");
}

// ============================================================================
// Bounce auto-disable: >= 5 hard bounces in 24h disables inbox
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_bounce_auto_disable_after_five_hard_bounces() {
    let pool = test_pool().await;
    let (org_id, _key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mut msg_ids = Vec::new();
    for i in 0..5 {
        let cm = create_message_input(inbox.id, &format!("Bounce {i}"), "body");
        let msg = postblox::db::messages::create(&pool, &cm).await.unwrap();
        msg_ids.push(msg.id);
    }

    for (i, &mid) in msg_ids.iter().enumerate() {
        let body = json!({
            "message_id": mid,
            "status": "bounced",
            "bounce_type": "hard",
            "details": {"smtp_code": 550}
        });
        let req = Request::post("/internal/stalwart/bounce")
            .header("authorization", "Bearer test-inbound-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "bounce {i} should succeed");
    }

    // Give async disable task a moment
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let refreshed = postblox::db::inboxes::get_by_id(&pool, inbox.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !refreshed.active,
        "inbox should be auto-disabled after 5 hard bounces"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_bounce_four_hard_bounces_does_not_disable() {
    let pool = test_pool().await;
    let (org_id, _key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    let mut msg_ids = Vec::new();
    for i in 0..4 {
        let cm = create_message_input(inbox.id, &format!("Bounce {i}"), "body");
        let msg = postblox::db::messages::create(&pool, &cm).await.unwrap();
        msg_ids.push(msg.id);
    }

    for &mid in &msg_ids {
        let body = json!({
            "message_id": mid,
            "status": "bounced",
            "bounce_type": "hard"
        });
        let req = Request::post("/internal/stalwart/bounce")
            .header("authorization", "Bearer test-inbound-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let refreshed = postblox::db::inboxes::get_by_id(&pool, inbox.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        refreshed.active,
        "inbox should still be active with only 4 hard bounces"
    );
}

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_bounce_soft_bounces_do_not_count_toward_disable() {
    let pool = test_pool().await;
    let (org_id, _key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let state = test_state_with_inbound(pool.clone());
    let app = test_app(state);

    for i in 0..6 {
        let cm = create_message_input(inbox.id, &format!("Soft {i}"), "body");
        let msg = postblox::db::messages::create(&pool, &cm).await.unwrap();
        let body = json!({
            "message_id": msg.id,
            "status": "bounced",
            "bounce_type": "soft"
        });
        let req = Request::post("/internal/stalwart/bounce")
            .header("authorization", "Bearer test-inbound-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        ServiceExt::oneshot(app.clone(), req).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let refreshed = postblox::db::inboxes::get_by_id(&pool, inbox.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        refreshed.active,
        "soft bounces should not trigger auto-disable"
    );
}

// ============================================================================
// DNS poller: list_pending query
// ============================================================================

#[tokio::test]
#[ignore] // requires DATABASE_URL
async fn test_list_pending_domains_returns_only_pending() {
    let pool = test_pool().await;
    let (org_id, _) = setup_admin_org(&pool).await;

    let pending_name = format!("{}.pending.test", Uuid::new_v4());
    let verified_name = format!("{}.verified.test", Uuid::new_v4());

    let pending = postblox::db::domains::create(&pool, org_id, &pending_name)
        .await
        .unwrap();
    let verified_d = postblox::db::domains::create(&pool, org_id, &verified_name)
        .await
        .unwrap();
    postblox::db::domains::set_verified(&pool, verified_d.id)
        .await
        .unwrap();

    let pending_list = postblox::db::domains::list_pending(&pool).await.unwrap();
    let ids: Vec<_> = pending_list.iter().map(|d| d.id).collect();
    assert!(ids.contains(&pending.id));
    assert!(
        !ids.contains(&verified_d.id),
        "verified domain should not be in pending list"
    );
}
