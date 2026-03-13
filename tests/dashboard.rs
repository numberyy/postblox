//! Integration tests for the dashboard HTTP routes.
//! Tests all 18 dashboard routes against real Postgres with Axum tower::ServiceExt.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use common::{create_message_input, setup_inbox, setup_org, test_app, test_pool, test_state};
use postblox::models::{CreateApproval, SendMode};

/// Make a GET request with cookie auth, return (status, body_string).
async fn dash_get(app: &axum::Router, path: &str, key: &str) -> (StatusCode, String) {
    let req = Request::get(path)
        .header("cookie", format!("postblox_key={key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

/// Make a GET request with query param auth (first visit), return (status, body, headers).
async fn dash_get_with_query_key(
    app: &axum::Router,
    path: &str,
    key: &str,
) -> (StatusCode, String, axum::http::HeaderMap) {
    let separator = if path.contains('?') { "&" } else { "?" };
    let full_path = format!("{path}{separator}key={key}");
    let req = Request::get(&full_path).body(Body::empty()).unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string(), headers)
}

/// Make an htmx POST (with hx-request header + cookie auth).
async fn dash_htmx_post(
    app: &axum::Router,
    path: &str,
    key: &str,
    form_body: &str,
) -> (StatusCode, String) {
    let req = Request::post(path)
        .header("cookie", format!("postblox_key={key}"))
        .header("hx-request", "true")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form_body.to_string()))
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

// --- Auth ---

#[tokio::test]
#[ignore]
async fn test_dashboard_unauthenticated_returns_401() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    let req = Request::get("/dashboard/inboxes")
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore]
async fn test_dashboard_valid_key_returns_200() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Inboxes") || body.contains("inbox"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_query_key_sets_cookie() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _body, headers) = dash_get_with_query_key(&app, "/dashboard/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);

    let cookie = headers
        .get("set-cookie")
        .expect("should set cookie on query key auth")
        .to_str()
        .unwrap();
    assert!(cookie.contains("postblox_key="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_cookie_auth_no_set_cookie() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let req = Request::get("/dashboard/inboxes")
        .header("cookie", format!("postblox_key={key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Cookie auth should NOT set a new cookie
    assert!(resp.headers().get("set-cookie").is_none());
}

// --- Index redirect ---

#[tokio::test]
#[ignore]
async fn test_dashboard_index_redirects_to_inboxes() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    // Router maps "/" (no trailing slash) to the index handler
    let req = Request::get("/dashboard").body(Body::empty()).unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        "/dashboard/inboxes"
    );
}

// --- Inboxes list ---

#[tokio::test]
#[ignore]
async fn test_dashboard_inboxes_list_renders_with_data() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&inbox.email), "should contain inbox email");
}

#[tokio::test]
#[ignore]
async fn test_dashboard_inboxes_list_shows_send_mode() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::permissions::upsert(&pool, inbox.id, SendMode::Autonomous, &json!([]))
        .await
        .unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, "/dashboard/inboxes", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("autonomous"));
}

// --- Inbox detail ---

#[tokio::test]
#[ignore]
async fn test_dashboard_inbox_detail_shows_messages() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Dashboard detail test", "Body here"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, &format!("/dashboard/inboxes/{}", inbox.id), &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Dashboard detail test"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_inbox_detail_wrong_org_returns_not_found() {
    let pool = test_pool().await;
    let (org1_id, _key1) = setup_org(&pool).await;
    let (_org2_id, key2) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org1_id).await;

    let app = test_app(test_state(pool));
    let (status, _) = dash_get(&app, &format!("/dashboard/inboxes/{}", inbox.id), &key2).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore]
async fn test_dashboard_inbox_detail_nonexistent_returns_not_found() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = dash_get(
        &app,
        &format!("/dashboard/inboxes/{}", Uuid::new_v4()),
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- Inbox messages partial (htmx pagination) ---

#[tokio::test]
#[ignore]
async fn test_dashboard_inbox_messages_partial() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    for i in 0..3 {
        postblox::db::messages::create(
            &pool,
            &create_message_input(inbox.id, &format!("Partial {i}"), &format!("Body {i}")),
        )
        .await
        .unwrap();
    }

    let app = test_app(test_state(pool));
    let path = format!("/dashboard/inboxes/{}/messages?limit=2&offset=0", inbox.id);
    let (status, body) = dash_get(&app, &path, &key).await;
    assert_eq!(status, StatusCode::OK);
    // Partial returns table rows, should contain message data
    assert!(body.contains("Partial"));
}

// --- Message detail ---

#[tokio::test]
#[ignore]
async fn test_dashboard_message_detail_renders() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Message detail test", "The full body content"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, &format!("/dashboard/messages/{}", msg.id), &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Message detail test"));
    assert!(body.contains("The full body content"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_message_detail_nonexistent() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = dash_get(
        &app,
        &format!("/dashboard/messages/{}", Uuid::new_v4()),
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- Thread view ---

#[tokio::test]
#[ignore]
async fn test_dashboard_thread_view_renders_conversation() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let thread = postblox::db::threads::create(&pool, inbox.id, Some("Thread topic"))
        .await
        .unwrap();

    let mut cm1 = create_message_input(inbox.id, "Thread topic", "First message");
    cm1.thread_id = Some(thread.id);
    postblox::db::messages::create(&pool, &cm1).await.unwrap();

    let mut cm2 = create_message_input(inbox.id, "Re: Thread topic", "Reply here");
    cm2.thread_id = Some(thread.id);
    postblox::db::messages::create(&pool, &cm2).await.unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, &format!("/dashboard/threads/{}", thread.id), &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("First message") || body.contains("Reply here"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_thread_view_nonexistent() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, _) = dash_get(
        &app,
        &format!("/dashboard/threads/{}", Uuid::new_v4()),
        &key,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- Approvals ---

#[tokio::test]
#[ignore]
async fn test_dashboard_approvals_page_lists_pending() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Needs approval", "Body"),
    )
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
    let (status, body) = dash_get(&app, "/dashboard/approvals", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Needs approval"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_approve_htmx_post() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Approve via dashboard", "Body"),
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
    let path = format!("/dashboard/approvals/{}/approve", approval.id);
    let (status, body) = dash_htmx_post(&app, &path, &key, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Approved"));

    // Verify in DB
    let fetched = postblox::db::approvals::get(&pool, org_id, approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.status, "approved");
    assert_eq!(fetched.decided_by.as_deref(), Some("dashboard"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_reject_htmx_post() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg = postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Reject via dashboard", "Body"),
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
    let path = format!("/dashboard/approvals/{}/reject", approval.id);
    let (status, body) = dash_htmx_post(&app, &path, &key, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Rejected"));

    let fetched = postblox::db::approvals::get(&pool, org_id, approval.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.status, "rejected");
}

#[tokio::test]
#[ignore]
async fn test_dashboard_approve_without_htmx_header_returns_403() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    let msg =
        postblox::db::messages::create(&pool, &create_message_input(inbox.id, "No htmx", "Body"))
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

    let app = test_app(test_state(pool));
    // POST without hx-request header
    let req = Request::post(format!("/dashboard/approvals/{}/approve", approval.id))
        .header("cookie", format!("postblox_key={key}"))
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// --- Briefing ---

#[tokio::test]
#[ignore]
async fn test_dashboard_briefing_renders_stats() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "Briefing email", "Content"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, "/dashboard/briefing", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("24h") || body.contains("Briefing") || body.contains("briefing"));
}

// --- Search ---

#[tokio::test]
#[ignore]
async fn test_dashboard_search_page_renders() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/search", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("search") || body.contains("Search"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_search_results_with_query() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;

    postblox::db::messages::create(
        &pool,
        &create_message_input(inbox.id, "unique findable subject", "searchable content"),
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let (status, body) = dash_get(&app, "/dashboard/search/results?q=findable", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("findable"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_search_results_empty_query() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/search/results?q=", &key).await;
    assert_eq!(status, StatusCode::OK);
    // Empty query should return no results
    assert!(!body.contains("<tr>") || body.is_empty() || body.len() < 200);
}

// --- Settings ---

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_page_renders() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/settings", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("settings") || body.contains("Settings") || body.contains("send_mode"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_change_mode() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool.clone()));

    let path = format!("/dashboard/settings/inbox/{}/mode", inbox.id);
    let (status, body) = dash_htmx_post(&app, &path, &key, "send_mode=autonomous").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("autonomous"));

    // Verify in DB
    let perm = postblox::db::permissions::get_by_inbox(&pool, inbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(perm.mode(), SendMode::Autonomous);
}

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_invalid_mode_returns_error() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;
    let inbox = setup_inbox(&pool, org_id).await;
    let app = test_app(test_state(pool));

    let path = format!("/dashboard/settings/inbox/{}/mode", inbox.id);
    let (status, body) = dash_htmx_post(&app, &path, &key, "send_mode=invalid_mode").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_create_notification() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let form = "provider=webhook&config=%7B%22url%22%3A%22https%3A%2F%2Fexample.com%2Fhook%22%7D";
    let (status, body) =
        dash_htmx_post(&app, "/dashboard/settings/notifications", &key, form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("webhook"));
    assert!(body.contains("example.com"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_create_notification_invalid_provider() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let form = "provider=invalid&config=%7B%7D";
    let (status, body) =
        dash_htmx_post(&app, "/dashboard/settings/notifications", &key, form).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_settings_delete_notification() {
    let pool = test_pool().await;
    let (org_id, key) = setup_org(&pool).await;

    let notif = postblox::db::notifications::create(
        &pool,
        &postblox::models::CreateNotificationConfig {
            org_id,
            provider: postblox::models::NotificationProvider::Webhook,
            config: json!({"url": "https://example.com/hook"}),
        },
    )
    .await
    .unwrap();

    let app = test_app(test_state(pool));
    let path = format!("/dashboard/settings/notifications/{}/delete", notif.id);
    let (status, body) = dash_htmx_post(&app, &path, &key, "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty()); // Deleted row returns empty
}

// --- Analytics ---

#[tokio::test]
#[ignore]
async fn test_dashboard_analytics_page_renders() {
    let pool = test_pool().await;
    let (_org_id, key) = setup_org(&pool).await;
    let app = test_app(test_state(pool));

    let (status, body) = dash_get(&app, "/dashboard/analytics", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("analytics") || body.contains("Analytics") || body.contains("slop"));
}

// --- Static assets ---

#[tokio::test]
#[ignore]
async fn test_dashboard_static_css_serves_with_correct_type() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    let req = Request::get("/dashboard/static/style.css")
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/css"
    );
    assert!(resp
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("max-age=86400"));
}

#[tokio::test]
#[ignore]
async fn test_dashboard_static_htmx_js_serves() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    let req = Request::get("/dashboard/static/htmx.min.js")
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/javascript"
    );
}

#[tokio::test]
#[ignore]
async fn test_dashboard_static_ws_js_serves() {
    let pool = test_pool().await;
    let app = test_app(test_state(pool));

    let req = Request::get("/dashboard/static/ws.js")
        .body(Body::empty())
        .unwrap();
    let resp = ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/javascript"
    );
    assert!(resp
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("public"));
}

// WebSocket upgrade tests omitted: tower::oneshot cannot perform real HTTP upgrades.
// The WebSocketUpgrade extractor returns 426 before auth is checked.
// WS auth is covered by unit tests in src/dashboard/ws.rs.

// --- Cross-org isolation ---

#[tokio::test]
#[ignore]
async fn test_dashboard_cross_org_inbox_isolation() {
    let pool = test_pool().await;
    let (org1_id, key1) = setup_org(&pool).await;
    let (_org2_id, key2) = setup_org(&pool).await;

    let inbox = setup_inbox(&pool, org1_id).await;

    let app = test_app(test_state(pool));

    // Org1 sees its inbox
    let (status1, body1) = dash_get(&app, "/dashboard/inboxes", &key1).await;
    assert_eq!(status1, StatusCode::OK);
    assert!(body1.contains(&inbox.email));

    // Org2 does NOT see org1's inbox
    let (status2, body2) = dash_get(&app, "/dashboard/inboxes", &key2).await;
    assert_eq!(status2, StatusCode::OK);
    assert!(!body2.contains(&inbox.email));
}
