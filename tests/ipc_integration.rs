//! End-to-end IPC test against the real `DaemonDispatcher` over a
//! real Unix socket and a real on-disk SQLite pool.

use std::sync::Arc;

use serde_json::json;
use sqlx::SqlitePool;
use tokio::time::{timeout, Duration};

use postblox::daemon::DaemonDispatcher;
use postblox::db::{accounts, connect, folders, messages, threads};
use postblox::imap::{FolderInfo, ImapAuth, ImapError};
use postblox::ipc::client::Client;
use postblox::ipc::{listen, Hub, Topic};
use postblox::models::{AuthKind, FolderRole};

struct Harness {
    _db_dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    sock: std::path::PathBuf,
    pool: SqlitePool,
    hub: Arc<Hub>,
    _server: postblox::ipc::server::ServerHandle,
}

async fn make_harness() -> Harness {
    make_harness_with_imap(Arc::new(NoImap)).await
}

async fn make_harness_with_imap(imap: Arc<dyn ImapAuth>) -> Harness {
    let db_dir = tempfile::tempdir().unwrap();
    let pool = connect(&db_dir.path().join("postblox.db")).await.unwrap();
    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher::with_imap(pool.clone(), hub.clone(), imap));
    let server = listen(&sock, dispatcher, hub.clone()).await.unwrap();
    Harness {
        _db_dir: db_dir,
        _sock_dir: sock_dir,
        sock,
        pool,
        hub,
        _server: server,
    }
}

/// Refuses every IMAP call. Default for tests that don't exercise the
/// network path; keeps the production rustls platform-verifier out of
/// `cargo test`.
struct NoImap;

#[async_trait::async_trait]
impl ImapAuth for NoImap {
    async fn test_login(
        &self,
        _: &str,
        _: u16,
        _: &str,
        _: &str,
    ) -> Result<Vec<FolderInfo>, ImapError> {
        Err(ImapError::Protocol(
            "imap not configured for this test".into(),
        ))
    }
}

fn account_args(email: &str) -> serde_json::Value {
    json!({
        "email": email,
        "display_name": null,
        "auth_kind": "password",
        "imap_host": "i",
        "imap_port": 993,
        "imap_use_tls": true,
        "smtp_host": "s",
        "smtp_port": 465,
        "smtp_use_tls": true,
        "smtp_starttls": false,
    })
}

#[tokio::test]
async fn account_create_then_list_round_trip() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let resp = c
        .request("account.create", account_args("alice@example.com"))
        .await
        .unwrap();
    assert!(resp.ok, "create failed: {:?}", resp.error);
    let id = resp.data["id"].as_str().unwrap().to_string();

    let listed = c.request("account.list", json!({})).await.unwrap();
    assert!(listed.ok);
    let arr = listed.data.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["email"], "alice@example.com");
    assert_eq!(arr[0]["id"], id);
}

#[tokio::test]
async fn account_create_with_bad_args_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request("account.create", json!({"email": "x"}))
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "bad_args");
}

#[tokio::test]
async fn folder_upsert_creates_then_updates() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let f1 = c
        .request(
            "folder.upsert",
            json!({
                "account_id": acc_id,
                "name": "INBOX",
                "delimiter": "/",
                "role": "inbox",
                "selectable": true,
            }),
        )
        .await
        .unwrap();
    assert!(f1.ok, "{:?}", f1.error);
    let folder_id = f1.data["id"].as_str().unwrap().to_string();

    // upsert again should keep the same row.
    let f2 = c
        .request(
            "folder.upsert",
            json!({
                "account_id": acc_id,
                "name": "INBOX",
                "delimiter": "/",
                "role": "inbox",
                "selectable": true,
            }),
        )
        .await
        .unwrap();
    assert_eq!(f2.data["id"].as_str().unwrap(), folder_id);
}

#[tokio::test]
async fn message_set_flags_publishes_mail_updated() {
    let h = make_harness().await;

    // Seed via direct db calls — set_flags needs an existing message.
    let acc = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: "a@x.com".into(),
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
    let folder = folders::upsert(
        &h.pool,
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
    let thread = threads::create(&h.pool, acc.id, None, Some("subj"))
        .await
        .unwrap();
    let msg = messages::create(
        &h.pool,
        &messages::NewMessage {
            account_id: acc.id,
            folder_id: folder.id,
            thread_id: Some(thread.id),
            uid: 1,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "b@x.com".into(),
            to_addrs: json!([]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("subj".into()),
            snippet: None,
            text_body: None,
            html_body: None,
            raw_size: 1,
            flags: json!([]),
            internal_date: chrono::Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let sub = c.subscribe(Topic::MailUpdated).await.unwrap();
    assert!(sub > 0);

    // Let the forwarder register before we publish.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resp = c
        .request(
            "message.set_flags",
            json!({"id": msg.id.to_string(), "flags": ["\\Seen"]}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);

    let event = timeout(Duration::from_secs(2), c.next_event())
        .await
        .expect("expected event")
        .unwrap();
    assert_eq!(event.topic, "mail.updated");
    assert_eq!(event.data["message_id"], msg.id.to_string());
    assert_eq!(event.data["flags"], json!(["\\Seen"]));

    // Audit log should have one entry for the flag change.
    let audit = c
        .request("audit.list_recent", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(audit.ok);
    let entries = audit.data.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "message.set_flags" && e["target"] == msg.id.to_string()),
        "expected audit entry for set_flags, got {entries:?}"
    );
}

#[tokio::test]
async fn draft_create_update_delete_round_trip() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "hi",
                "text_body": "hello",
                "html_body": null,
                "in_reply_to_msg": null,
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let draft_id = created.data["id"].as_str().unwrap().to_string();

    let updated = c
        .request(
            "draft.update",
            json!({
                "id": draft_id,
                "to_addrs": ["bob@x.com"],
                "subject": "edited",
                "text_body": "edited body",
            }),
        )
        .await
        .unwrap();
    assert!(updated.ok, "{:?}", updated.error);
    assert_eq!(updated.data["subject"], "edited");
    assert_eq!(updated.data["text_body"], "edited body");

    let deleted = c
        .request("draft.delete", json!({"id": draft_id}))
        .await
        .unwrap();
    assert!(deleted.ok);
    assert_eq!(deleted.data["removed"], true);
}

#[tokio::test]
async fn account_delete_cascade_through_socket() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let created = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = created.data["id"].as_str().unwrap().to_string();

    let removed = c
        .request("account.delete", json!({"id": acc_id}))
        .await
        .unwrap();
    assert!(removed.ok);
    assert_eq!(removed.data["removed"], true);

    let listed = c.request("account.list", json!({})).await.unwrap();
    assert!(listed.data.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn folder_list_with_bad_uuid_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request("folder.list", json!({"account_id": "not-a-uuid"}))
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.error.unwrap().code, "bad_args");
}

#[tokio::test]
async fn subscription_delivers_published_event_via_hub() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let sub = c.subscribe(Topic::MailNew).await.unwrap();
    assert!(sub > 0);

    tokio::time::sleep(Duration::from_millis(20)).await;
    h.hub.publish(Topic::MailNew, json!({"id": "abc"})).await;

    let event = timeout(Duration::from_secs(2), c.next_event())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.topic, "mail.new");
    assert_eq!(event.data, json!({"id": "abc"}));
}

#[tokio::test]
async fn many_concurrent_clients_all_get_responses() {
    let h = make_harness().await;
    for email in ["a@x.com", "b@x.com"] {
        accounts::create(
            &h.pool,
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
    }

    let mut handles = Vec::new();
    for _ in 0..10 {
        let sock = h.sock.clone();
        handles.push(tokio::spawn(async move {
            let mut c = Client::connect(&sock).await.unwrap();
            for _ in 0..5 {
                let resp = c.request("account.list", json!({})).await.unwrap();
                assert!(resp.ok);
                assert_eq!(resp.data.as_array().unwrap().len(), 2);
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

// ---- IMAP test_login -------------------------------------------------------

/// `ImapAuth` mock with deterministic behaviour: returns the list of
/// folders unless the password matches `bad_password`, in which case it
/// reports an auth failure.
struct MockImap {
    folders: Vec<FolderInfo>,
    bad_password: String,
}

#[async_trait::async_trait]
impl ImapAuth for MockImap {
    async fn test_login(
        &self,
        _: &str,
        _: u16,
        _: &str,
        password: &str,
    ) -> Result<Vec<FolderInfo>, ImapError> {
        if password == self.bad_password {
            return Err(ImapError::Auth("bad creds".into()));
        }
        Ok(self.folders.clone())
    }
}

fn fi(name: &str) -> FolderInfo {
    FolderInfo {
        name: name.into(),
        delimiter: "/".into(),
        selectable: true,
    }
}

#[tokio::test]
async fn account_test_login_seeds_folders_with_role_mapping() {
    let mock = Arc::new(MockImap {
        folders: vec![
            fi("INBOX"),
            fi("Sent"),
            fi("Drafts"),
            fi("[Gmail]/All Mail"),
            fi("Notes"),
        ],
        bad_password: "WRONG".into(),
    });
    let h = make_harness_with_imap(mock).await;

    let account = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: "user@example.com".into(),
            display_name: None,
            auth_kind: AuthKind::Password,
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.test_login",
            json!({"account_id": account.id, "password": "hunter2"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "expected ok, got {:?}", resp);
    assert_eq!(resp.data["ok"], json!(true));
    assert_eq!(resp.data["folders"].as_array().unwrap().len(), 5);

    let got = folders::list_by_account(&h.pool, account.id).await.unwrap();
    let by_name = |n: &str| got.iter().find(|f| f.name == n).expect("folder present");
    assert_eq!(by_name("INBOX").role, FolderRole::Inbox);
    assert_eq!(by_name("Sent").role, FolderRole::Sent);
    assert_eq!(by_name("Drafts").role, FolderRole::Drafts);
    assert_eq!(by_name("[Gmail]/All Mail").role, FolderRole::All);
    assert_eq!(by_name("Notes").role, FolderRole::Custom);
}

#[tokio::test]
async fn account_test_login_returns_auth_failed_on_bad_password() {
    let mock = Arc::new(MockImap {
        folders: vec![fi("INBOX")],
        bad_password: "WRONG".into(),
    });
    let h = make_harness_with_imap(mock).await;

    let account = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: "user@example.com".into(),
            display_name: None,
            auth_kind: AuthKind::Password,
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.test_login",
            json!({"account_id": account.id, "password": "WRONG"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("auth_failed")
    );
    // No folders should have been written.
    let got = folders::list_by_account(&h.pool, account.id).await.unwrap();
    assert!(got.is_empty());
}

#[tokio::test]
async fn account_test_login_unknown_account_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.test_login",
            json!({
                "account_id": uuid::Uuid::new_v4(),
                "password": "x",
            }),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}
