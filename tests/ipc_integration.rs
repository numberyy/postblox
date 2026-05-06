//! End-to-end IPC test against the real `DaemonDispatcher` over a
//! real Unix socket and a real on-disk SQLite pool.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use sqlx::SqlitePool;
use tokio::time::{timeout, Duration};

use postblox::daemon::DaemonDispatcher;
use postblox::db::{accounts, connect, folders, messages, threads};
use postblox::imap::{FetchedMessage, FolderInfo, FolderSync, ImapAuth, ImapError, ImapSync};
use postblox::ipc::client::Client;
use postblox::ipc::{listen, Hub, Topic};
use postblox::models::{AuthKind, FolderRole};
use postblox::secrets::{
    file::{FileSecretStore, KdfParams},
    SecretStore,
};
use postblox::smtp::{SmtpError, SmtpSubmitRequest, SmtpSubmitter};
use postblox::sync::WorkerConfig;

struct Harness {
    _db_dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    sock: std::path::PathBuf,
    pool: SqlitePool,
    hub: Arc<Hub>,
    _server: postblox::ipc::server::ServerHandle,
}

async fn make_harness() -> Harness {
    make_harness_with(Arc::new(NoImap), Arc::new(NoSync)).await
}

async fn make_harness_with_imap(imap: Arc<dyn ImapAuth>) -> Harness {
    make_harness_with(imap, Arc::new(NoSync)).await
}

async fn make_harness_with_sync(sync: Arc<dyn ImapSync>) -> Harness {
    make_harness_with(Arc::new(NoImap), sync).await
}

async fn make_harness_with(imap: Arc<dyn ImapAuth>, imap_sync: Arc<dyn ImapSync>) -> Harness {
    make_harness_with_config(imap, imap_sync, WorkerConfig::default()).await
}

async fn make_harness_with_sync_config(
    sync: Arc<dyn ImapSync>,
    worker_config: WorkerConfig,
) -> Harness {
    make_harness_with_config(Arc::new(NoImap), sync, worker_config).await
}

async fn make_harness_with_config(
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    worker_config: WorkerConfig,
) -> Harness {
    make_harness_with_config_and_smtp(
        imap,
        imap_sync,
        Arc::new(postblox::smtp::LettreSmtpSubmitter::new()),
        worker_config,
    )
    .await
}

async fn make_harness_with_smtp(smtp: Arc<dyn SmtpSubmitter>) -> Harness {
    make_harness_with_config_and_smtp(
        Arc::new(NoImap),
        Arc::new(NoSync),
        smtp,
        WorkerConfig::default(),
    )
    .await
}

async fn make_harness_with_config_and_smtp(
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    smtp: Arc<dyn SmtpSubmitter>,
    worker_config: WorkerConfig,
) -> Harness {
    let db_dir = tempfile::tempdir().unwrap();
    let pool = connect(&db_dir.path().join("postblox.db")).await.unwrap();
    let sock_dir = tempfile::tempdir().unwrap();
    let sock = sock_dir.path().join("postbloxd.sock");
    let hub = Arc::new(Hub::new());
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::with_params(
        db_dir.path().join("secrets.bin"),
        "test-passphrase",
        KdfParams::insecure_for_tests(),
    ));
    let dispatcher = Arc::new(DaemonDispatcher::with_imap_sync_smtp_config(
        pool.clone(),
        hub.clone(),
        imap,
        imap_sync,
        secrets,
        smtp,
        worker_config,
    ));
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

/// Sync counterpart of `NoImap`.
struct NoSync;

#[async_trait::async_trait]
impl ImapSync for NoSync {
    async fn sync_folder(
        &self,
        _: &str,
        _: u16,
        _: &str,
        _: &str,
        _: &str,
        _: u32,
    ) -> Result<FolderSync, ImapError> {
        Err(ImapError::Protocol(
            "imap sync not configured for this test".into(),
        ))
    }
}

#[derive(Debug, Clone)]
struct CapturedSmtp {
    host: String,
    port: u16,
    use_tls: bool,
    starttls: bool,
    username: String,
    password: String,
    from: String,
    recipients: Vec<String>,
    mime: Vec<u8>,
}

struct MockSmtp {
    calls: Mutex<Vec<CapturedSmtp>>,
    outcomes: Mutex<VecDeque<Result<(), SmtpError>>>,
}

impl MockSmtp {
    fn ok() -> Self {
        Self {
            calls: Mutex::new(vec![]),
            outcomes: Mutex::new(VecDeque::new()),
        }
    }

    fn with_outcome(outcome: Result<(), SmtpError>) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            outcomes: Mutex::new(VecDeque::from([outcome])),
        }
    }

    fn calls(&self) -> Vec<CapturedSmtp> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl SmtpSubmitter for MockSmtp {
    async fn submit(&self, request: SmtpSubmitRequest) -> Result<(), SmtpError> {
        self.calls.lock().unwrap().push(CapturedSmtp {
            host: request.server.host,
            port: request.server.port,
            use_tls: request.server.use_tls,
            starttls: request.server.starttls,
            username: request.username,
            password: request.password.to_string(),
            from: request.from,
            recipients: request.recipients,
            mime: request.mime,
        });
        self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()))
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
async fn message_send_with_draft_submits_mime_and_audits() {
    let smtp = Arc::new(MockSmtp::ok());
    let h = make_harness_with_smtp(smtp.clone()).await;
    let account_id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let draft = c
        .request(
            "draft.create",
            json!({
                "account_id": account_id,
                "to_addrs": ["to@example.com"],
                "cc_addrs": ["copy@example.com"],
                "bcc_addrs": ["blind@example.com"],
                "subject": "SMTP hi",
                "text_body": "plain body",
                "html_body": "<p>html body</p>",
                "in_reply_to_msg": null,
            }),
        )
        .await
        .unwrap();
    assert!(draft.ok, "{:?}", draft.error);
    let draft_id = draft.data["id"].as_str().unwrap().to_string();

    let sent = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft_id}),
        )
        .await
        .unwrap();
    assert!(sent.ok, "{:?}", sent.error);
    let message_id = sent.data["message_id"].as_str().unwrap();

    let calls = smtp.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.host, "smtp.example.com");
    assert_eq!(call.port, 465);
    assert!(call.use_tls);
    assert!(!call.starttls);
    assert_eq!(call.username, "u@example.com");
    assert_eq!(call.password, "right");
    assert_eq!(call.from, "u@example.com");
    assert_eq!(
        call.recipients,
        vec![
            "to@example.com".to_string(),
            "copy@example.com".to_string(),
            "blind@example.com".to_string()
        ]
    );
    let mime = String::from_utf8(call.mime.clone()).unwrap();
    assert!(mime.contains("From: u@example.com\r\n"));
    assert!(mime.contains("To: to@example.com\r\n"));
    assert!(mime.contains("Cc: copy@example.com\r\n"));
    assert!(mime.contains("Subject: SMTP hi\r\n"));
    assert!(mime.contains(&format!("Message-ID: {message_id}\r\n")));
    assert!(mime.contains("plain body"));
    assert!(mime.contains("<p>html body</p>"));
    assert!(!mime.contains("blind@example.com"));

    let audit = c
        .request("audit.list_recent", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(audit.ok);
    let entries = audit.data.as_array().unwrap();
    assert!(
        entries.iter().any(|e| {
            e["action"] == "message.send"
                && e["target"] == draft_id
                && e["details"]["message_id"] == message_id
        }),
        "expected message.send audit entry, got {entries:?}"
    );
}

#[tokio::test]
async fn message_send_missing_secret_returns_missing_secret() {
    let smtp = Arc::new(MockSmtp::ok());
    let h = make_harness_with_smtp(smtp.clone()).await;
    let account_id = make_account(&h, "u@example.com").await;
    let draft = postblox::db::drafts::create(
        &h.pool,
        &postblox::db::drafts::NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: json!(["to@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            subject: Some("No secret".into()),
            text_body: Some("body".into()),
            html_body: None,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft.id}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("missing_secret")
    );
    assert!(smtp.calls().is_empty());
}

#[tokio::test]
async fn message_send_auth_failure_maps_to_auth_failed() {
    let smtp = Arc::new(MockSmtp::with_outcome(Err(SmtpError::Auth(
        "bad credentials".into(),
    ))));
    let h = make_harness_with_smtp(smtp.clone()).await;
    let account_id = setup_account_with_secret(&h).await;
    let draft = postblox::db::drafts::create(
        &h.pool,
        &postblox::db::drafts::NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: json!(["to@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            subject: Some("Auth".into()),
            text_body: Some("body".into()),
            html_body: None,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft.id}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("auth_failed")
    );
    assert_eq!(smtp.calls().len(), 1);
}

#[tokio::test]
async fn message_send_bad_args_return_bad_args() {
    let smtp = Arc::new(MockSmtp::ok());
    let h = make_harness_with_smtp(smtp.clone()).await;
    let account_id = setup_account_with_secret(&h).await;
    let draft = postblox::db::drafts::create(
        &h.pool,
        &postblox::db::drafts::NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: json!([]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            subject: Some("No recipients".into()),
            text_body: Some("body".into()),
            html_body: None,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft.id}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
    assert!(smtp.calls().is_empty());
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

// ---- account.set_secret / delete_secret ------------------------------------

async fn make_account(h: &Harness, email: &str) -> uuid::Uuid {
    accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: email.into(),
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
    .unwrap()
    .id
}

#[tokio::test]
async fn account_set_secret_round_trip_via_socket() {
    let h = make_harness().await;
    let id = make_account(&h, "u@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let resp = c
        .request(
            "account.set_secret",
            json!({"account_id": id, "password": "hunter2"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp);
    assert_eq!(resp.data["ok"], json!(true));

    // Confirm the audit row was written so the daemon can prove it.
    let audits = postblox::db::audit::list_recent(&h.pool, 10, 0)
        .await
        .unwrap();
    assert!(audits.iter().any(|a| a.action == "account.set_secret"));
}

#[tokio::test]
async fn account_set_secret_rejects_unknown_account() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.set_secret",
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

#[tokio::test]
async fn account_set_secret_rejects_empty_password() {
    let h = make_harness().await;
    let id = make_account(&h, "u@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.set_secret",
            json!({"account_id": id, "password": ""}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn account_delete_secret_is_idempotent() {
    let h = make_harness().await;
    let id = make_account(&h, "u@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    // delete before set: should still succeed
    let resp = c
        .request("account.delete_secret", json!({"account_id": id}))
        .await
        .unwrap();
    assert!(resp.ok);

    // set, then delete
    c.request(
        "account.set_secret",
        json!({"account_id": id, "password": "p"}),
    )
    .await
    .unwrap();
    let resp = c
        .request("account.delete_secret", json!({"account_id": id}))
        .await
        .unwrap();
    assert!(resp.ok);

    // delete again: still ok
    let resp = c
        .request("account.delete_secret", json!({"account_id": id}))
        .await
        .unwrap();
    assert!(resp.ok);
}

// ---- account.sync_folder ---------------------------------------------------

/// Fetcher with scripted UID validity / message list. Per-call password
/// check so we can also exercise the auth-failure path.
struct ScriptedSync {
    uid_validity: std::sync::Mutex<u32>,
    uid_next: std::sync::Mutex<u32>,
    messages: std::sync::Mutex<Vec<FetchedMessage>>,
    password: String,
}

impl ScriptedSync {
    fn new(uid_validity: u32, uid_next: u32, msgs: Vec<FetchedMessage>) -> Self {
        Self {
            uid_validity: std::sync::Mutex::new(uid_validity),
            uid_next: std::sync::Mutex::new(uid_next),
            messages: std::sync::Mutex::new(msgs),
            password: "right".into(),
        }
    }
}

#[async_trait::async_trait]
impl ImapSync for ScriptedSync {
    async fn sync_folder(
        &self,
        _: &str,
        _: u16,
        _: &str,
        password: &str,
        _: &str,
        from_uid: u32,
    ) -> Result<FolderSync, ImapError> {
        if password != self.password {
            return Err(ImapError::Auth("bad password".into()));
        }
        let messages: Vec<FetchedMessage> = self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.uid >= from_uid)
            .cloned()
            .collect();
        Ok(FolderSync {
            uid_validity: Some(*self.uid_validity.lock().unwrap()),
            uid_next: Some(*self.uid_next.lock().unwrap()),
            exists: messages.len() as u32,
            messages,
        })
    }
}

fn rfc822(msg_id: &str, in_reply_to: Option<&str>, subject: &str, body: &str) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!("Message-ID: <{msg_id}>\r\n"));
    if let Some(p) = in_reply_to {
        s.push_str(&format!("In-Reply-To: <{p}>\r\n"));
        s.push_str(&format!("References: <{p}>\r\n"));
    }
    s.push_str("From: alice@example.com\r\n");
    s.push_str("To: bob@example.com\r\n");
    s.push_str(&format!("Subject: {subject}\r\n"));
    s.push_str("MIME-Version: 1.0\r\n");
    s.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    s.push_str("\r\n");
    s.push_str(body);
    s.into_bytes()
}

async fn setup_account_with_secret(h: &Harness) -> uuid::Uuid {
    let id = make_account(h, "u@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.set_secret",
            json!({"account_id": id, "password": "right"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp);
    id
}

fn fast_worker_config() -> WorkerConfig {
    WorkerConfig {
        poll_interval: Duration::from_millis(50),
        idle_timeout: Duration::from_secs(30),
        initial_backoff: Duration::from_millis(5),
        max_backoff: Duration::from_millis(10),
    }
}

async fn wait_for_message_count(
    pool: &SqlitePool,
    account_id: uuid::Uuid,
    folder_name: &str,
    expected: usize,
) {
    timeout(Duration::from_secs(2), async {
        loop {
            let folder = folders::get_by_name(pool, account_id, folder_name)
                .await
                .unwrap()
                .unwrap();
            let rows = messages::list_by_folder(pool, folder.id, 100, 0)
                .await
                .unwrap();
            if rows.len() >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected messages inserted by sync worker");
}

#[tokio::test]
async fn account_start_sync_starts_worker_and_stop_sync_is_idempotent() {
    let msgs = vec![FetchedMessage {
        uid: 11,
        flags: vec![],
        internal_date: Some(chrono::Utc::now()),
        raw: rfc822("worker@x", None, "Worker", "body"),
    }];
    let h = make_harness_with_sync_config(
        Arc::new(ScriptedSync::new(1, 12, msgs)),
        fast_worker_config(),
    )
    .await;
    let account_id = setup_account_with_secret(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let started = c
        .request(
            "account.start_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(started.ok, "{:?}", started);
    assert_eq!(started.data["ok"], json!(true));
    assert_eq!(started.data["started"], json!(true));

    wait_for_message_count(&h.pool, account_id, "INBOX", 1).await;

    let duplicate = c
        .request(
            "account.start_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(duplicate.ok, "{:?}", duplicate);
    assert_eq!(duplicate.data["started"], json!(false));

    let stopped = c
        .request(
            "account.stop_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(stopped.ok, "{:?}", stopped);
    assert_eq!(stopped.data["stopped"], json!(true));

    let stopped_again = c
        .request(
            "account.stop_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(stopped_again.ok, "{:?}", stopped_again);
    assert_eq!(stopped_again.data["stopped"], json!(false));
}

#[tokio::test]
async fn account_start_sync_missing_secret_returns_missing_secret() {
    let h = make_harness_with_sync_config(
        Arc::new(ScriptedSync::new(1, 1, vec![])),
        fast_worker_config(),
    )
    .await;
    let id = make_account(&h, "missing@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.start_sync",
            json!({"account_id": id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("missing_secret")
    );
}

#[tokio::test]
async fn account_start_sync_unknown_account_or_folder_returns_bad_args() {
    let h = make_harness_with_sync_config(
        Arc::new(ScriptedSync::new(1, 1, vec![])),
        fast_worker_config(),
    )
    .await;
    let id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let missing_account = c
        .request(
            "account.start_sync",
            json!({"account_id": uuid::Uuid::new_v4(), "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(!missing_account.ok);
    assert_eq!(
        missing_account.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );

    let missing_folder = c
        .request(
            "account.start_sync",
            json!({"account_id": id, "folder_name": "Nope"}),
        )
        .await
        .unwrap();
    assert!(!missing_folder.ok);
    assert_eq!(
        missing_folder.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn account_sync_folder_inserts_new_messages_and_publishes_events() {
    let msgs = vec![
        FetchedMessage {
            uid: 5,
            flags: vec!["\\Seen".into()],
            internal_date: Some(chrono::Utc::now()),
            raw: rfc822("a1@x", None, "Hello", "world"),
        },
        FetchedMessage {
            uid: 6,
            flags: vec![],
            internal_date: Some(chrono::Utc::now()),
            raw: rfc822("a2@x", Some("a1@x"), "Re: Hello", "again"),
        },
    ];
    let h = make_harness_with_sync(Arc::new(ScriptedSync::new(1, 7, msgs))).await;
    let account_id = setup_account_with_secret(&h).await;

    // subscribe to mail.new before triggering the sync so we see events
    let mut sub = Client::connect(&h.sock).await.unwrap();
    let _sub_id = sub.subscribe(Topic::MailNew).await.unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp);
    assert_eq!(resp.data["inserted"], json!(2));
    assert_eq!(resp.data["wiped"], json!(0));

    // both messages on disk
    let folder = folders::get_by_name(&h.pool, account_id, "INBOX")
        .await
        .unwrap()
        .unwrap();
    let rows = messages::list_by_folder(&h.pool, folder.id, 100, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    let uids: Vec<i64> = rows.iter().map(|m| m.uid).collect();
    assert!(uids.contains(&5));
    assert!(uids.contains(&6));

    // both replies threaded into one thread (In-Reply-To on the second)
    let thread_ids: std::collections::HashSet<_> = rows.iter().map(|m| m.thread_id).collect();
    assert_eq!(
        thread_ids.len(),
        1,
        "expected single thread, got {thread_ids:?}"
    );

    // last_seen_uid was updated to the high UID
    let folder_after = folders::get(&h.pool, folder.id).await.unwrap().unwrap();
    assert_eq!(folder_after.last_seen_uid, Some(6));
    assert_eq!(folder_after.uid_validity, Some(1));
    assert_eq!(folder_after.uid_next, Some(7));

    // Two mail.new events (one per inserted message). Use a generous
    // timeout because the dispatcher publishes after each insert.
    timeout(Duration::from_secs(2), async {
        let _ = sub.next_event().await.unwrap();
        let _ = sub.next_event().await.unwrap();
    })
    .await
    .expect("mail.new events");
}

#[tokio::test]
async fn account_sync_folder_skips_already_present_uids() {
    let msgs = vec![FetchedMessage {
        uid: 1,
        flags: vec![],
        internal_date: Some(chrono::Utc::now()),
        raw: rfc822("dup@x", None, "Hi", "dup"),
    }];
    let scripted = Arc::new(ScriptedSync::new(1, 2, msgs));
    let h = make_harness_with_sync(scripted.clone()).await;
    let account_id = setup_account_with_secret(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    // First call inserts.
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.data["inserted"], json!(1));

    // Second call: same UID still on the server, but already in DB.
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.data["inserted"], json!(0));

    // exactly one row
    let folder = folders::get_by_name(&h.pool, account_id, "INBOX")
        .await
        .unwrap()
        .unwrap();
    let rows = messages::list_by_folder(&h.pool, folder.id, 100, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn account_sync_folder_uid_validity_change_triggers_full_resync() {
    let scripted = Arc::new(ScriptedSync::new(
        1,
        2,
        vec![FetchedMessage {
            uid: 1,
            flags: vec![],
            internal_date: Some(chrono::Utc::now()),
            raw: rfc822("first@x", None, "first", "body1"),
        }],
    ));
    let h = make_harness_with_sync(scripted.clone()).await;
    let account_id = setup_account_with_secret(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok);

    // Server rolls UIDVALIDITY and presents an entirely new mailbox.
    *scripted.uid_validity.lock().unwrap() = 99;
    *scripted.uid_next.lock().unwrap() = 2;
    *scripted.messages.lock().unwrap() = vec![FetchedMessage {
        uid: 1,
        flags: vec![],
        internal_date: Some(chrono::Utc::now()),
        raw: rfc822("brand_new@x", None, "different mailbox", "x"),
    }];

    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp);
    assert_eq!(resp.data["wiped"], json!(1), "expected wipe");
    assert_eq!(resp.data["inserted"], json!(1));

    // The original message is gone; only the new one remains.
    let folder = folders::get_by_name(&h.pool, account_id, "INBOX")
        .await
        .unwrap()
        .unwrap();
    let rows = messages::list_by_folder(&h.pool, folder.id, 100, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id_header.as_deref(), Some("brand_new@x"));

    // uid_validity is now the new value
    let folder_after = folders::get(&h.pool, folder.id).await.unwrap().unwrap();
    assert_eq!(folder_after.uid_validity, Some(99));
}

#[tokio::test]
async fn account_sync_folder_empty_inbox_is_a_noop() {
    let h = make_harness_with_sync(Arc::new(ScriptedSync::new(1, 1, vec![]))).await;
    let account_id = setup_account_with_secret(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.data["inserted"], json!(0));
    assert_eq!(resp.data["wiped"], json!(0));
}

#[tokio::test]
async fn account_sync_folder_returns_auth_failed_on_bad_password() {
    // Stored password is "wrong" but ScriptedSync expects "right"
    let h = make_harness_with_sync(Arc::new(ScriptedSync::new(1, 1, vec![]))).await;
    let id = make_account(&h, "u@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();
    let mut c = Client::connect(&h.sock).await.unwrap();
    c.request(
        "account.set_secret",
        json!({"account_id": id, "password": "wrong"}),
    )
    .await
    .unwrap();

    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("auth_failed")
    );
}

#[tokio::test]
async fn account_sync_folder_unknown_folder_returns_bad_args() {
    let h = make_harness_with_sync(Arc::new(ScriptedSync::new(1, 1, vec![]))).await;
    let id = make_account(&h, "u@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    c.request(
        "account.set_secret",
        json!({"account_id": id, "password": "right"}),
    )
    .await
    .unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": id, "folder_name": "Nope"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn account_sync_folder_missing_secret_surfaces_specific_error() {
    let h = make_harness_with_sync(Arc::new(ScriptedSync::new(1, 1, vec![]))).await;
    let id = make_account(&h, "u@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("missing_secret")
    );
}
