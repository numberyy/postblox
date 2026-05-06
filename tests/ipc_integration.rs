//! End-to-end IPC test: real Unix socket, real SQLite pool, real
//! `Dispatcher` impl. Covers what the daemon binary itself runs.

use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::SqlitePool;

use postblox::db::{accounts, connect, folders};
use postblox::ipc::client::Client;
use postblox::ipc::{listen, Dispatcher, Hub, RpcError, Topic};
use postblox::models::{AuthKind, FolderRole};

#[derive(Clone)]
struct DaemonDispatcher {
    pool: SqlitePool,
}

#[async_trait::async_trait]
impl Dispatcher for DaemonDispatcher {
    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError> {
        match op {
            "account.list" => serde_json::to_value(
                accounts::list(&self.pool)
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?,
            )
            .map_err(|e| RpcError::internal(e.to_string())),
            "folder.list" => {
                let id = args
                    .get("account_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::bad_args("missing account_id"))?;
                let id =
                    uuid::Uuid::parse_str(id).map_err(|e| RpcError::bad_args(e.to_string()))?;
                serde_json::to_value(
                    folders::list_by_account(&self.pool, id)
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?,
                )
                .map_err(|e| RpcError::internal(e.to_string()))
            }
            other => Err(RpcError::unknown_op(other)),
        }
    }
}

async fn make_pool() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("postblox.db");
    let pool = connect(&path).await.unwrap();
    (dir, pool)
}

#[tokio::test]
async fn account_list_round_trip_through_socket() {
    let (_dir, pool) = make_pool().await;

    let acc = accounts::create(
        &pool,
        &accounts::NewAccount {
            email: "alice@example.com".into(),
            display_name: Some("Alice".into()),
            auth_kind: AuthKind::Password,
            imap_host: "i".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "s".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        },
    )
    .await
    .unwrap();

    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher { pool: pool.clone() });
    let _server = listen(&sock, dispatcher, hub).await.unwrap();

    let mut client = Client::connect(&sock).await.unwrap();

    let resp = client.request("account.list", json!({})).await.unwrap();
    assert!(resp.ok, "account.list should succeed: {:?}", resp.error);
    let arr = resp.data.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["email"], "alice@example.com");
    assert_eq!(arr[0]["id"], acc.id.to_string());
}

#[tokio::test]
async fn folder_list_with_bad_uuid_returns_bad_args() {
    let (_dir, pool) = make_pool().await;
    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher { pool });
    let _server = listen(&sock, dispatcher, hub).await.unwrap();

    let mut client = Client::connect(&sock).await.unwrap();
    let resp = client
        .request("folder.list", json!({"account_id": "not-a-uuid"}))
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "bad_args");
}

#[tokio::test]
async fn subscription_delivers_published_event() {
    let (_dir, pool) = make_pool().await;
    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher { pool: pool.clone() });
    let _server = listen(&sock, dispatcher, hub.clone()).await.unwrap();

    let mut client = Client::connect(&sock).await.unwrap();
    let sub = client.subscribe(Topic::MailNew).await.unwrap();
    assert!(sub > 0);

    // Allow the forwarder task to register before we publish.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    hub.publish(Topic::MailNew, json!({"id": "abc"})).await;

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), client.next_event())
        .await
        .expect("event must arrive")
        .unwrap();
    assert_eq!(event.topic, "mail.new");
    assert_eq!(event.data, json!({"id": "abc"}));
}

#[tokio::test]
async fn many_concurrent_clients_all_get_responses() {
    let (_dir, pool) = make_pool().await;

    // Insert two accounts + a folder per account so list has content.
    for email in ["a@x.com", "b@x.com"] {
        let acc = accounts::create(
            &pool,
            &accounts::NewAccount {
                email: email.into(),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "i".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        folders::upsert(
            &pool,
            &folders::NewFolder {
                account_id: acc.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
    }

    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher { pool: pool.clone() });
    let _server = listen(&sock, dispatcher, hub).await.unwrap();

    // 10 concurrent clients, each issuing 5 ops.
    let mut handles = Vec::new();
    for _ in 0..10 {
        let sock = sock.clone();
        handles.push(tokio::spawn(async move {
            let mut client = Client::connect(&sock).await.unwrap();
            for _ in 0..5 {
                let resp = client.request("account.list", json!({})).await.unwrap();
                assert!(resp.ok);
                assert_eq!(resp.data.as_array().unwrap().len(), 2);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}
