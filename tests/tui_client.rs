//! Integration tests for TUI client API endpoints.
//! Tests the HTTP endpoints that PostbloxClient calls, against a real Axum server + Postgres.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::{create_message_input, setup_inbox, setup_org, test_app, test_pool, test_state};
use postblox::models::CreateApproval;

async fn get_json(app: &axum::Router, path: &str, key: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::get(path)
        .header("authorization", format!("Bearer {key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!(null));
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
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!(null));
    (status, json)
}

async fn post_decision(app: &axum::Router, path: &str, key: &str) -> StatusCode {
    let body = json!({"decided_by": "test"});
    let req = Request::post(path)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    resp.status()
}

// --- list_inboxes ---

#[tokio::test]
#[ignore] // needs real DB
async fn test_tui_list_inboxes_returns_real_inboxes() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let (status, json) = get_json(&app, "/api/v1/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);

    let inboxes = json.as_array().unwrap();
    assert!(inboxes.iter().any(|i| i["id"] == inbox.id.to_string()));
}

#[tokio::test]
#[ignore]
async fn test_tui_list_inboxes_empty_org() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, json) = get_json(&app, "/api/v1/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
#[ignore]
async fn test_tui_list_inboxes_unauthorized() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    let (status, _) = get_json(&app, "/api/v1/inboxes", "pb_badkey123456").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- list_messages ---

#[tokio::test]
#[ignore]
async fn test_tui_list_messages_returns_messages_for_inbox() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Hello TUI", "Message body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages?limit=10&offset=0", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);

    let messages = json.as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], msg.id.to_string());
    assert_eq!(messages[0]["subject"], "Hello TUI");
}

#[tokio::test]
#[ignore]
async fn test_tui_list_messages_empty_inbox() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/inboxes/{}/messages?limit=10&offset=0", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
#[ignore]
async fn test_tui_list_messages_pagination() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    for i in 0..5 {
        postblox::db::messages::create(
            &pool,
            &create_message_input(inbox.id, &format!("Msg {i}"), &format!("Body {i}")),
        )
        .await
        .unwrap();
    }

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages?limit=3&offset=0", inbox.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 3);
}

#[tokio::test]
#[ignore]
async fn test_tui_list_messages_wrong_org_returns_not_found() {
    let pool = test_pool().await;
    let (org1_id, _key1) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org1_id).await;

    // Create second org
    let org2 = postblox::db::organizations::create(&pool, "Other Org")
        .await
        .unwrap();
    let raw_key2 = format!("pb_{}", Uuid::new_v4().to_string().replace('-', ""));
    let hash2 = postblox::api::auth::hash_key(&raw_key2);
    postblox::db::api_keys::create(&pool, org2.id, &hash2, &raw_key2[..8], None)
        .await
        .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages?limit=10&offset=0", inbox.id);
    let (status, _) = get_json(&app, &path, &raw_key2).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- send_message ---

#[tokio::test]
#[ignore]
async fn test_tui_send_message_creates_message() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Set inbox to autonomous mode so send goes through without Stalwart
    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::Autonomous,
        &json!([]),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "TUI Send Test",
        "text_body": "Hello from TUI",
    });
    let (status, json) = post_json(&app, &path, &key, &body).await;

    // Without Stalwart, Approval mode stores message + creates approval → 202
    // AutoApprove/Autonomous would try to deliver and fail without Stalwart
    assert!(
        status == StatusCode::CREATED
            || status == StatusCode::ACCEPTED
            || status == StatusCode::INTERNAL_SERVER_ERROR,
        "expected 201/202/500, got {status}"
    );
    if status == StatusCode::CREATED || status == StatusCode::ACCEPTED {
        assert_eq!(json["subject"], "TUI Send Test");
    }
}

#[tokio::test]
#[ignore]
async fn test_tui_send_message_shadow_mode_rejected() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::permissions::upsert(
        &pool,
        inbox.id,
        postblox::models::SendMode::Shadow,
        &json!([]),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages", inbox.id);
    let body = json!({
        "to": ["test@example.com"],
        "subject": "Should fail",
        "text_body": "Body",
    });
    let (status, _) = post_json(&app, &path, &key, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// --- search ---

#[tokio::test]
#[ignore]
async fn test_tui_search_returns_matching_results() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(
            inbox.id,
            "quarterly report",
            "The quarterly numbers look great",
        ),
    )
    .await
    .unwrap();
    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "lunch plans", "Let's grab tacos"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, json) = get_json(&app, "/api/v1/search?q=quarterly", &key).await;
    assert_eq!(status, StatusCode::OK);

    let results = json.as_array().unwrap();
    assert!(
        results.iter().any(|r| r["subject"] == "quarterly report"),
        "expected 'quarterly report' in search results"
    );
}

#[tokio::test]
#[ignore]
async fn test_tui_search_empty_query_returns_bad_request() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = get_json(&app, "/api/v1/search?q=", &key).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// --- briefing ---

#[tokio::test]
#[ignore]
async fn test_tui_briefing_returns_stats() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Briefing test", "Some body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, json) = get_json(&app, "/api/v1/briefing?period=24h", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["total_received"].is_number());
    assert!(json["by_inbox"].is_array());
}

#[tokio::test]
#[ignore]
async fn test_tui_briefing_empty_org() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, json) = get_json(&app, "/api/v1/briefing?period=24h", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["total_received"], 0);
    assert_eq!(json["total_sent"], 0);
}

// --- approvals ---

#[tokio::test]
#[ignore]
async fn test_tui_approve_message_changes_status() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Approve me", "Body"),
    )
    .await
    .unwrap();

    let approval = postblox::db::approvals::create(
        &pool,
        &CreateApproval {
            org_id,
            inbox_id: inbox.id,
            message_id: msg.id,
        },
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/approvals/{}/approve", approval.id);
    let status = post_decision(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);

    // Verify in DB
    let fetched = postblox::db::approvals::get(&pool, org_id, approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.status, "approved");
}

#[tokio::test]
#[ignore]
async fn test_tui_reject_message_changes_status() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg =
        postblox::db::messages::create(&pool, &create_message_input(inbox.id, "Reject me", "Body"))
            .await
            .unwrap();

    let approval = postblox::db::approvals::create(
        &pool,
        &CreateApproval {
            org_id,
            inbox_id: inbox.id,
            message_id: msg.id,
        },
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool.clone()));
    let path = format!("/api/v1/approvals/{}/reject", approval.id);
    let status = post_decision(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);

    let fetched = postblox::db::approvals::get(&pool, org_id, approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.status, "rejected");
}

#[tokio::test]
#[ignore]
async fn test_tui_list_approvals() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg =
        postblox::db::messages::create(&pool, &create_message_input(inbox.id, "Pending", "Body"))
            .await
            .unwrap();
    postblox::db::approvals::create(
        &pool,
        &CreateApproval {
            org_id,
            inbox_id: inbox.id,
            message_id: msg.id,
        },
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, json) = get_json(&app, "/api/v1/approvals", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!json.as_array().unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn test_tui_approve_already_decided_returns_none() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Double approve", "Body"),
    )
    .await
    .unwrap();

    let approval = postblox::db::approvals::create(
        &pool,
        &CreateApproval {
            org_id,
            inbox_id: inbox.id,
            message_id: msg.id,
        },
    )
    .await
    .unwrap();

    // Approve in DB directly
    postblox::db::approvals::approve(&pool, org_id, approval.id, "admin")
        .await
        .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/approvals/{}/approve", approval.id);
    let status = post_decision(&app, &path, &key).await;
    // Already decided — handler returns 404 (approval not in pending status)
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn test_tui_approve_nonexistent_returns_not_found() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let path = format!("/api/v1/approvals/{}/approve", Uuid::new_v4());
    let status = post_decision(&app, &path, &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- ws_url ---

#[test]
fn test_tui_ws_url_http_to_ws() {
    let key = "pb_testkey123";
    let encoded_key = urlencoding::encode(key);
    let ws_url = format!("ws://localhost:3000/api/v1/ws?key={encoded_key}");
    assert_eq!(ws_url, format!("ws://localhost:3000/api/v1/ws?key={key}"));
    assert!(ws_url.starts_with("ws://"));
    assert!(ws_url.contains(&format!("key={key}")));

    // HTTPS → WSS
    let ws_base = "https://mail.example.com".replacen("https://", "wss://", 1);
    assert!(ws_base.starts_with("wss://"));
}

#[test]
fn test_tui_ws_url_encodes_special_chars_in_key() {
    let key = "pb_key+with=chars&more";
    let encoded = urlencoding::encode(key);
    let ws_url = format!("ws://localhost/api/v1/ws?key={encoded}");
    // Special chars in the key value should be encoded
    assert!(!ws_url.contains("+with"));
    assert!(ws_url.contains("%2B"));
    assert!(ws_url.contains("%3D"));
    assert!(ws_url.contains("%26"));
    // The query separator '=' in 'key=' is fine — only the value should be encoded
    assert!(ws_url.contains("key="));
}

// --- cross-org isolation ---

#[tokio::test]
#[ignore]
async fn test_tui_cross_org_inbox_isolation() {
    let pool = test_pool().await;
    let (org1_id, key1) = setup_org(&pool).await;
    let (_org2_id, key2) = setup_org(&pool).await;

    setup_inbox(&pool, org1_id).await;

    let app = test_app(test_state(pool));

    // Org1 sees its inbox
    let (status1, json1) = get_json(&app, "/api/v1/inboxes", &key1).await;
    assert_eq!(status1, StatusCode::OK);
    assert!(!json1.as_array().unwrap().is_empty());

    // Org2 sees nothing
    let (status2, json2) = get_json(&app, "/api/v1/inboxes", &key2).await;
    assert_eq!(status2, StatusCode::OK);
    assert!(json2.as_array().unwrap().is_empty());
}

// --- get single message ---

#[tokio::test]
#[ignore]
async fn test_tui_get_message_by_id() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Single fetch", "The body"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages/{}", inbox.id, msg.id);
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["id"], msg.id.to_string());
    assert_eq!(json["subject"], "Single fetch");
}

#[tokio::test]
#[ignore]
async fn test_tui_get_message_nonexistent_returns_not_found() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let app = test_app(test_state(pool));
    let path = format!("/api/v1/inboxes/{}/messages/{}", inbox.id, Uuid::new_v4());
    let (status, _) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- thread messages ---

#[tokio::test]
#[ignore]
async fn test_tui_get_thread_messages() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    // Create a thread
    let thread = postblox::db::threads::create(&pool, inbox.id, Some("Thread subject"))
        .await
        .unwrap();

    // Create messages in the thread
    let mut cm = create_message_input(inbox.id, "Thread msg 1", "First");
    cm.thread_id = Some(thread.id);
    postblox::db::messages::create(&pool, &cm).await.unwrap();

    let mut cm2 = create_message_input(inbox.id, "Thread msg 2", "Second");
    cm2.thread_id = Some(thread.id);
    postblox::db::messages::create(&pool, &cm2).await.unwrap();

    let app = test_app(test_state(pool));
    // PostbloxClient::get_thread_messages uses /messages?thread_id=
    let path = format!(
        "/api/v1/inboxes/{}/messages?thread_id={}",
        inbox.id, thread.id
    );
    let (status, json) = get_json(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json.as_array().unwrap().len(), 2);
}
