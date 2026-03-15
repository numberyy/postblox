mod common;

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
            disposition: "attachment".into(),
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
