//! End-to-end IPC test against the real `DaemonDispatcher` over a
//! real Unix socket and a real on-disk SQLite pool.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mockall::mock;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::time::{timeout, Duration};

use postblox::auth::{CredentialKind, MailCredential};
use postblox::daemon::{worker_manager_with_idle_config, DaemonDispatcher, DaemonServices};
use postblox::db::{accounts, attachments as db_attachments, connect, folders, messages, threads};
use postblox::imap::{FetchedMessage, FolderInfo, FolderSync, ImapAuth, ImapError, ImapSync};
use postblox::ipc::client::Client;
use postblox::ipc::{listen, Hub, Topic};
use postblox::models::{
    AccountId, ApprovalState, AttachmentDisposition, AuthKind, DraftId, FolderId, FolderRole,
    MessageId, ThreadId,
};
use postblox::oauth::google::{
    self, GoogleOAuth, GoogleOAuthConfig, GoogleOAuthError, GoogleOAuthToken,
};
use postblox::secrets::{
    file::{FileSecretStore, KdfParams},
    SecretStore,
};
use postblox::smtp::{SmtpError, SmtpSubmitRequest, SmtpSubmitter};
use postblox::sync::WorkerConfig;

mock! {
    ImapAuthMock {}
    #[async_trait::async_trait]
    impl ImapAuth for ImapAuthMock {
        async fn test_login(
            &self,
            host: &str,
            port: u16,
            username: &str,
            credential: &MailCredential,
        ) -> Result<Vec<FolderInfo>, ImapError>;
    }
}

mock! {
    ImapSyncMock {}
    #[async_trait::async_trait]
    impl ImapSync for ImapSyncMock {
        async fn sync_folder(
            &self,
            host: &str,
            port: u16,
            username: &str,
            credential: &MailCredential,
            folder: &str,
            from_uid: u32,
        ) -> Result<FolderSync, ImapError>;
    }
}

mock! {
    GoogleOAuthMock {}
    #[async_trait::async_trait]
    impl GoogleOAuth for GoogleOAuthMock {
        async fn exchange_code(
            &self,
            config: &GoogleOAuthConfig,
            code: &str,
        ) -> Result<GoogleOAuthToken, GoogleOAuthError>;
        async fn refresh_token(
            &self,
            config: &GoogleOAuthConfig,
            token: &GoogleOAuthToken,
        ) -> Result<GoogleOAuthToken, GoogleOAuthError>;
    }
}

mock! {
    SmtpSubmitterMock {}
    #[async_trait::async_trait]
    impl SmtpSubmitter for SmtpSubmitterMock {
        async fn submit(&self, request: SmtpSubmitRequest) -> Result<(), SmtpError>;
    }
}

struct Harness {
    _db_dir: tempfile::TempDir,
    _sock_dir: tempfile::TempDir,
    sock: std::path::PathBuf,
    pool: SqlitePool,
    hub: Arc<Hub>,
    secrets: Arc<dyn SecretStore>,
    _server: postblox::ipc::server::ServerHandle,
}

async fn make_harness() -> Harness {
    make_harness_with(no_imap(), no_sync()).await
}

async fn make_harness_with_imap(imap: Arc<dyn ImapAuth>) -> Harness {
    make_harness_with(imap, no_sync()).await
}

async fn make_harness_with_sync(sync: Arc<dyn ImapSync>) -> Harness {
    make_harness_with(no_imap(), sync).await
}

async fn make_harness_with(imap: Arc<dyn ImapAuth>, imap_sync: Arc<dyn ImapSync>) -> Harness {
    make_harness_with_config(imap, imap_sync, WorkerConfig::default()).await
}

async fn make_harness_with_sync_config(
    sync: Arc<dyn ImapSync>,
    worker_config: WorkerConfig,
) -> Harness {
    make_harness_with_config(no_imap(), sync, worker_config).await
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
    make_harness_with_config_and_smtp(no_imap(), no_sync(), smtp, WorkerConfig::default()).await
}

async fn make_harness_with_config_and_smtp(
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    smtp: Arc<dyn SmtpSubmitter>,
    worker_config: WorkerConfig,
) -> Harness {
    make_harness_with_config_smtp_oauth(
        imap,
        imap_sync,
        smtp,
        Arc::new(MockGoogleOAuthMock::new()),
        worker_config,
    )
    .await
}

async fn make_harness_with_config_smtp_oauth(
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    smtp: Arc<dyn SmtpSubmitter>,
    oauth: Arc<dyn GoogleOAuth>,
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
    let services = DaemonServices::new(smtp, oauth);
    let worker_manager = worker_manager_with_idle_config(
        &pool,
        &hub,
        imap_sync.clone(),
        None,
        &secrets,
        &services,
        worker_config,
    );
    let dispatcher = Arc::new(DaemonDispatcher::with_imap_smtp_oauth_and_manager(
        pool.clone(),
        // In-memory SQLite has no separate RO connection; reuse the
        // same pool. The keyword scan is the actual safety net.
        pool.clone(),
        hub.clone(),
        imap,
        imap_sync,
        secrets.clone(),
        services,
        worker_manager,
    ));
    let server = listen(&sock, dispatcher, hub.clone()).await.unwrap();
    Harness {
        _db_dir: db_dir,
        _sock_dir: sock_dir,
        sock,
        pool,
        hub,
        secrets,
        _server: server,
    }
}

/// Refuses every IMAP call. Default for tests that don't exercise the
/// network path; keeps the production rustls platform-verifier out of
/// `cargo test`.
fn no_imap() -> Arc<dyn ImapAuth> {
    let mut mock = MockImapAuthMock::new();
    mock.expect_test_login().returning(|_, _, _, _| {
        Err(ImapError::Protocol(
            "imap not configured for this test".into(),
        ))
    });
    Arc::new(mock)
}

/// Sync counterpart of [`no_imap`].
fn no_sync() -> Arc<dyn ImapSync> {
    let mut mock = MockImapSyncMock::new();
    mock.expect_sync_folder().returning(|_, _, _, _, _, _| {
        Err(ImapError::Protocol(
            "imap sync not configured for this test".into(),
        ))
    });
    Arc::new(mock)
}

fn google_oauth_with_exchange(token: GoogleOAuthToken) -> Arc<dyn GoogleOAuth> {
    google_oauth_with_exchange_and_refreshes(token, vec![])
}

fn google_oauth_with_exchange_and_refresh(
    exchange: GoogleOAuthToken,
    refresh: GoogleOAuthToken,
) -> Arc<dyn GoogleOAuth> {
    google_oauth_with_exchange_and_refreshes(exchange, vec![refresh])
}

fn google_oauth_with_exchange_and_refreshes(
    exchange: GoogleOAuthToken,
    refreshes: Vec<GoogleOAuthToken>,
) -> Arc<dyn GoogleOAuth> {
    let exchange_queue: Arc<Mutex<VecDeque<Result<GoogleOAuthToken, GoogleOAuthError>>>> =
        Arc::new(Mutex::new(VecDeque::from([Ok(exchange)])));
    let refresh_queue: Arc<Mutex<VecDeque<Result<GoogleOAuthToken, GoogleOAuthError>>>> =
        Arc::new(Mutex::new(refreshes.into_iter().map(Ok).collect()));
    let mut mock = MockGoogleOAuthMock::new();
    mock.expect_exchange_code().returning(move |_, _| {
        exchange_queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(GoogleOAuthError::InvalidInput("no exchange token".into())))
    });
    mock.expect_refresh_token().returning(move |_, _| {
        refresh_queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err(GoogleOAuthError::InvalidInput("no refresh token".into())))
    });
    Arc::new(mock)
}

#[derive(Debug, Clone)]
struct CapturedSmtp {
    host: String,
    port: u16,
    use_tls: bool,
    starttls: bool,
    username: String,
    credential_kind: CredentialKind,
    secret: String,
    from: String,
    recipients: Vec<String>,
    mime: Vec<u8>,
}

/// Wraps a mockall-driven [`SmtpSubmitter`] together with a shared call
/// log so tests can assert on the captured requests without losing the
/// mockall expectation API.
struct MockSmtp {
    inner: Arc<dyn SmtpSubmitter>,
    calls: Arc<Mutex<Vec<CapturedSmtp>>>,
}

impl MockSmtp {
    fn ok() -> Self {
        Self::new(VecDeque::new())
    }

    fn with_outcome(outcome: Result<(), SmtpError>) -> Self {
        Self::new(VecDeque::from([outcome]))
    }

    fn new(outcomes: VecDeque<Result<(), SmtpError>>) -> Self {
        let calls: Arc<Mutex<Vec<CapturedSmtp>>> = Arc::new(Mutex::new(vec![]));
        let outcomes: Arc<Mutex<VecDeque<Result<(), SmtpError>>>> = Arc::new(Mutex::new(outcomes));
        let calls_for_mock = Arc::clone(&calls);
        let mut mock = MockSmtpSubmitterMock::new();
        mock.expect_submit().returning(move |request| {
            calls_for_mock.lock().unwrap().push(CapturedSmtp {
                host: request.server.host,
                port: request.server.port,
                use_tls: request.server.use_tls,
                starttls: request.server.starttls,
                username: request.username,
                credential_kind: request.credential.kind(),
                secret: request.credential.secret().to_string(),
                from: request.from,
                recipients: request.recipients,
                mime: request.mime,
            });
            outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()))
        });
        Self {
            inner: Arc::new(mock),
            calls,
        }
    }

    fn submitter(&self) -> Arc<dyn SmtpSubmitter> {
        Arc::clone(&self.inner)
    }

    fn calls(&self) -> Vec<CapturedSmtp> {
        self.calls.lock().unwrap().clone()
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
    let set_flags_count = entries
        .iter()
        .filter(|e| e["action"] == "message.set_flags" && e["target"] == msg.id.to_string())
        .count();
    assert!(
        set_flags_count > 0,
        "expected audit entry for set_flags, got {entries:?}"
    );

    let repeat = c
        .request(
            "message.set_flags",
            json!({"id": msg.id.to_string(), "flags": ["\\Seen"]}),
        )
        .await
        .unwrap();
    assert!(repeat.ok, "{:?}", repeat.error);
    assert!(
        timeout(Duration::from_millis(100), c.next_event())
            .await
            .is_err(),
        "no-op set_flags unexpectedly published mail.updated"
    );

    let audit = c
        .request("audit.list_recent", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(audit.ok);
    let entries = audit.data.as_array().unwrap();
    let repeat_set_flags_count = entries
        .iter()
        .filter(|e| e["action"] == "message.set_flags" && e["target"] == msg.id.to_string())
        .count();
    assert_eq!(
        repeat_set_flags_count, set_flags_count,
        "no-op set_flags unexpectedly created an audit entry: {entries:?}"
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
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
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
    assert_eq!(call.credential_kind, CredentialKind::Password);
    assert_eq!(call.secret, "right");
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
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
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
            in_reply_to: None,
            references_header: None,
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
    let smtp = MockSmtp::with_outcome(Err(SmtpError::Auth("bad credentials".into())));
    let h = make_harness_with_smtp(smtp.submitter()).await;
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
            in_reply_to: None,
            references_header: None,
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
async fn message_send_oauth_account_uses_bearer_token_for_smtp() {
    let smtp = MockSmtp::ok();
    let oauth = google_oauth_with_exchange(oauth_token("smtp-access", "smtp-refresh", 3600));
    let h = make_harness_with_config_smtp_oauth(
        no_imap(),
        no_sync(),
        smtp.submitter(),
        oauth,
        WorkerConfig::default(),
    )
    .await;
    let account_id = make_oauth_account(&h, "gmail@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let complete = c
        .request(
            "oauth.google.complete",
            json!({
                "account_id": account_id,
                "client_id": "client",
                "client_secret": "secret",
                "redirect_uri": "http://127.0.0.1/callback",
                "code": "code",
                "state": "state",
                "expected_state": "state",
            }),
        )
        .await
        .unwrap();
    assert!(complete.ok, "{:?}", complete.error);
    let draft = postblox::db::drafts::create(
        &h.pool,
        &postblox::db::drafts::NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: json!(["to@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            subject: Some("OAuth SMTP".into()),
            text_body: Some("body".into()),
            html_body: None,
            in_reply_to: None,
            references_header: None,
        },
    )
    .await
    .unwrap();

    let sent = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft.id}),
        )
        .await
        .unwrap();
    assert!(sent.ok, "{:?}", sent.error);

    let calls = smtp.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].username, "gmail@example.com");
    assert_eq!(calls[0].credential_kind, CredentialKind::OAuth2Bearer);
    assert_eq!(calls[0].secret, "smtp-access");
}

#[tokio::test]
async fn message_send_bad_args_return_bad_args() {
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
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
            in_reply_to: None,
            references_header: None,
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
async fn mcp_gate_ops_round_trip_over_socket() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let created = c
        .request(
            "mcp.gate.create",
            json!({
                "tool": "postblox_message_send",
                "arg_pattern": json!({
                    "account_id": "00000000-0000-0000-0000-000000000001"
                })
                .to_string(),
                "action": "auto_allow",
                "note": "test rule",
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    assert_eq!(created.data["tool"], "postblox_message_send");
    assert_eq!(created.data["action"], "auto_allow");
    let gate_id = created.data["id"].as_str().unwrap().to_string();

    let listed = c
        .request("mcp.gate.list", json!({"tool": "postblox_message_send"}))
        .await
        .unwrap();
    assert!(listed.ok, "{:?}", listed.error);
    let gates = listed.data.as_array().unwrap();
    assert_eq!(gates.len(), 1);
    assert_eq!(gates[0]["id"], gate_id);

    let other_tool = c
        .request("mcp.gate.list", json!({"tool": "postblox_draft_delete"}))
        .await
        .unwrap();
    assert!(other_tool.ok);
    assert!(other_tool.data.as_array().unwrap().is_empty());

    let removed = c
        .request("mcp.gate.delete", json!({"id": gate_id}))
        .await
        .unwrap();
    assert!(removed.ok);
    assert_eq!(removed.data["removed"], true);
    assert!(postblox::db::mcp::list_gates(&h.pool)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn mcp_approval_ops_publish_requested_and_decided_events() {
    let h = make_harness().await;
    let mut events = Client::connect(&h.sock).await.unwrap();
    let _requested_sub = events.subscribe(Topic::McpApprovalRequested).await.unwrap();
    let _decided_sub = events.subscribe(Topic::McpApprovalDecided).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let created = c
        .request(
            "mcp.approval.create",
            json!({
                "tool": "postblox_message_send",
                "args": {"draft_id": "00000000-0000-0000-0000-000000000001"},
                "summary": "send test draft",
                "_actor": "mcp:postblox_message_send",
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let approval_id = created.data["id"].as_str().unwrap().to_string();
    assert_eq!(created.data["state"], "pending");

    let requested = timeout(Duration::from_secs(2), events.next_event())
        .await
        .expect("approval requested event")
        .unwrap();
    assert_eq!(requested.topic, "mcp.approval_requested");
    assert_eq!(requested.data["approval_id"], approval_id);
    assert_eq!(requested.data["tool"], "postblox_message_send");

    let listed = c
        .request("mcp.approval.list", json!({"state": "pending"}))
        .await
        .unwrap();
    assert!(listed.ok);
    assert_eq!(listed.data.as_array().unwrap().len(), 1);

    let got = c
        .request("mcp.approval.get", json!({"id": approval_id}))
        .await
        .unwrap();
    assert!(got.ok);
    assert_eq!(got.data["state"], "pending");

    let decided = c
        .request(
            "mcp.approval.decide",
            json!({
                "id": approval_id,
                "state": "allowed",
                "decided_by": "test-user",
            }),
        )
        .await
        .unwrap();
    assert!(decided.ok);
    assert_eq!(decided.data["decided"], true);

    let event = timeout(Duration::from_secs(2), events.next_event())
        .await
        .expect("approval decided event")
        .unwrap();
    assert_eq!(event.topic, "mcp.approval_decided");
    assert_eq!(event.data["approval_id"], approval_id);
    assert_eq!(event.data["state"], "allowed");

    let approval =
        postblox::db::mcp::get_approval(&h.pool, uuid::Uuid::parse_str(&approval_id).unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(approval.state, ApprovalState::Allowed);
    assert_eq!(approval.decided_by.as_deref(), Some("test-user"));
}

#[tokio::test]
async fn mcp_actor_override_is_used_for_write_audit_rows() {
    let h = make_harness().await;
    let account_id = make_account(&h, "mcp-audit@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": account_id,
                "to_addrs": ["bob@example.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "audit",
                "text_body": "body",
                "html_body": null,
                "in_reply_to_msg": null,
                "_actor": "mcp:postblox_draft_create",
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);

    let audits = postblox::db::audit::list_recent(&h.pool, 10, 0)
        .await
        .unwrap();
    assert!(audits.iter().any(|entry| {
        entry.action == "draft.create" && entry.actor == "mcp:postblox_draft_create"
    }));
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
fn imap_auth_with(folders: Vec<FolderInfo>, bad_password: &str) -> Arc<dyn ImapAuth> {
    let bad_password = bad_password.to_string();
    let mut mock = MockImapAuthMock::new();
    mock.expect_test_login()
        .returning(move |_, _, _, credential| {
            if credential.secret() == bad_password {
                return Err(ImapError::Auth("bad creds".into()));
            }
            Ok(folders.clone())
        });
    Arc::new(mock)
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
    let mock = imap_auth_with(
        vec![
            fi("INBOX"),
            fi("Sent"),
            fi("Drafts"),
            fi("[Gmail]/All Mail"),
            fi("Notes"),
        ],
        "WRONG",
    );
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
    let mock = imap_auth_with(vec![fi("INBOX")], "WRONG");
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
                "account_id": AccountId::new(),
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

async fn make_account(h: &Harness, email: &str) -> AccountId {
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

async fn make_oauth_account(h: &Harness, email: &str) -> AccountId {
    accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: email.into(),
            display_name: None,
            auth_kind: AuthKind::OAuth2Google,
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        },
    )
    .await
    .unwrap()
    .id
}

fn oauth_token(access_token: &str, refresh_token: &str, expires_in: i64) -> GoogleOAuthToken {
    GoogleOAuthToken {
        access_token: access_token.into(),
        refresh_token: refresh_token.into(),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in),
        token_type: "Bearer".into(),
        scope: Some("https://mail.google.com/".into()),
    }
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
                "account_id": AccountId::new(),
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

// ---- OAuth Google setup ----------------------------------------------------

#[tokio::test]
async fn oauth_google_auth_url_and_complete_store_token_without_auditing_secrets() {
    let oauth = google_oauth_with_exchange(oauth_token("access-token", "refresh-token", 3600));
    let h = make_harness_with_config_smtp_oauth(
        no_imap(),
        no_sync(),
        MockSmtp::ok().submitter(),
        oauth,
        WorkerConfig::default(),
    )
    .await;
    let account_id = make_oauth_account(&h, "gmail@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let auth = c
        .request(
            "oauth.google.auth_url",
            json!({
                "account_id": account_id,
                "client_id": "client id",
                "redirect_uri": "http://127.0.0.1/callback",
                "state": "state 1",
            }),
        )
        .await
        .unwrap();
    assert!(auth.ok, "{:?}", auth.error);
    let url = auth.data["authorization_url"].as_str().unwrap();
    assert!(url.contains("client_id=client%20id"));
    assert!(url.contains("state=state%201"));
    assert!(url.contains("scope=https%3A%2F%2Fmail.google.com%2F"));

    let complete = c
        .request(
            "oauth.google.complete",
            json!({
                "account_id": account_id,
                "client_id": "client id",
                "client_secret": "client-secret",
                "redirect_uri": "http://127.0.0.1/callback",
                "code": "code-123",
                "state": "state 1",
                "expected_state": "state 1",
            }),
        )
        .await
        .unwrap();
    assert!(complete.ok, "{:?}", complete.error);
    assert_eq!(complete.data["ok"], json!(true));

    let stored = google::load_stored_oauth(h.secrets.as_ref(), account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.client_id, "client id");
    assert_eq!(stored.client_secret, "client-secret");
    assert_eq!(stored.token.access_token, "access-token");
    assert_eq!(stored.token.refresh_token, "refresh-token");

    let account = accounts::get(&h.pool, account_id).await.unwrap().unwrap();
    assert!(account.secret_ref.is_some());

    let audits = postblox::db::audit::list_recent(&h.pool, 10, 0)
        .await
        .unwrap();
    assert!(audits.iter().any(|a| a.action == "oauth.google.complete"));
    let audit_json = serde_json::to_string(&audits).unwrap();
    assert!(!audit_json.contains("client-secret"));
    assert!(!audit_json.contains("code-123"));
    assert!(!audit_json.contains("access-token"));
    assert!(!audit_json.contains("refresh-token"));
}

#[tokio::test]
async fn oauth_google_complete_rejects_state_mismatch_and_password_account() {
    let h = make_harness().await;
    let password_account = make_account(&h, "u@example.com").await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let mismatch = c
        .request(
            "oauth.google.complete",
            json!({
                "account_id": password_account,
                "client_id": "client",
                "client_secret": "secret",
                "redirect_uri": "http://127.0.0.1/callback",
                "code": "code",
                "state": "actual",
                "expected_state": "expected",
            }),
        )
        .await
        .unwrap();
    assert!(!mismatch.ok);
    assert_eq!(
        mismatch.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );

    let wrong_kind = c
        .request(
            "oauth.google.auth_url",
            json!({
                "account_id": password_account,
                "client_id": "client",
                "redirect_uri": "http://127.0.0.1/callback",
                "state": "state",
            }),
        )
        .await
        .unwrap();
    assert!(!wrong_kind.ok);
    assert_eq!(
        wrong_kind.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

// ---- account.sync_folder ---------------------------------------------------

/// Wrapper around a mockall-driven [`ImapSync`] that exposes shared
/// state (UID validity, UID next, message list) so individual tests can
/// mutate them between calls. Per-call credential check exercises the
/// auth-failure path.
struct ScriptedSync {
    uid_validity: Arc<Mutex<u32>>,
    uid_next: Arc<Mutex<u32>>,
    messages: Arc<Mutex<Vec<FetchedMessage>>>,
    inner: Arc<dyn ImapSync>,
}

impl ScriptedSync {
    fn new(uid_validity: u32, uid_next: u32, msgs: Vec<FetchedMessage>) -> Self {
        Self::with_credential(
            uid_validity,
            uid_next,
            msgs,
            CredentialKind::Password,
            "right",
        )
    }

    fn with_credential(
        uid_validity: u32,
        uid_next: u32,
        msgs: Vec<FetchedMessage>,
        expected_kind: CredentialKind,
        secret: &str,
    ) -> Self {
        let uid_validity = Arc::new(Mutex::new(uid_validity));
        let uid_next = Arc::new(Mutex::new(uid_next));
        let messages = Arc::new(Mutex::new(msgs));
        let password = secret.to_string();
        let mut mock = MockImapSyncMock::new();
        {
            let uid_validity = Arc::clone(&uid_validity);
            let uid_next = Arc::clone(&uid_next);
            let messages = Arc::clone(&messages);
            mock.expect_sync_folder()
                .returning(move |_, _, _, credential, _, from_uid| {
                    assert_eq!(credential.kind(), expected_kind);
                    if credential.secret() != password {
                        return Err(ImapError::Auth("bad password".into()));
                    }
                    let messages: Vec<FetchedMessage> = messages
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|m| m.uid >= from_uid)
                        .cloned()
                        .collect();
                    Ok(FolderSync {
                        uid_validity: Some(*uid_validity.lock().unwrap()),
                        uid_next: Some(*uid_next.lock().unwrap()),
                        exists: messages.len() as u32,
                        messages,
                    })
                });
        }
        Self {
            uid_validity,
            uid_next,
            messages,
            inner: Arc::new(mock),
        }
    }

    fn syncer(&self) -> Arc<dyn ImapSync> {
        Arc::clone(&self.inner)
    }
}

/// Records the secrets passed to each `sync_folder` call so tests can
/// verify the worker refreshed credentials between cycles.
struct RecordingSync {
    secrets: Arc<Mutex<Vec<String>>>,
    inner: Arc<dyn ImapSync>,
}

impl Default for RecordingSync {
    fn default() -> Self {
        let secrets: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let secrets_for_mock = Arc::clone(&secrets);
        let mut mock = MockImapSyncMock::new();
        mock.expect_sync_folder()
            .returning(move |_, _, _, credential, _, _| {
                secrets_for_mock
                    .lock()
                    .unwrap()
                    .push(credential.secret().to_string());
                Ok(FolderSync {
                    uid_validity: Some(1),
                    uid_next: Some(1),
                    exists: 0,
                    messages: vec![],
                })
            });
        Self {
            secrets,
            inner: Arc::new(mock),
        }
    }
}

impl RecordingSync {
    fn syncer(&self) -> Arc<dyn ImapSync> {
        Arc::clone(&self.inner)
    }

    fn secrets(&self) -> Vec<String> {
        self.secrets.lock().unwrap().clone()
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

fn rfc822_with_text_attachment(msg_id: &str, filename: &str, attachment_text: &str) -> Vec<u8> {
    use base64::Engine;

    let encoded = base64::engine::general_purpose::STANDARD.encode(attachment_text.as_bytes());
    format!(
        "Message-ID: <{msg_id}>\r\n\
From: alice@example.com\r\n\
To: bob@example.com\r\n\
Subject: Attachment\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Body\r\n\
--b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Disposition: attachment; filename=\"{filename}\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
{encoded}\r\n\
--b--\r\n"
    )
    .into_bytes()
}

async fn setup_account_with_secret(h: &Harness) -> AccountId {
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
    account_id: AccountId,
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

async fn wait_for_recorded_secrets(sync: &RecordingSync, expected: usize) -> Vec<String> {
    timeout(Duration::from_secs(2), async {
        loop {
            let secrets = sync.secrets();
            if secrets.len() >= expected {
                return secrets;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expected sync worker credentials")
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
        ScriptedSync::new(1, 12, msgs).syncer(),
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
async fn account_start_sync_oauth_worker_refreshes_between_poll_cycles() {
    let oauth = google_oauth_with_exchange_and_refreshes(
        oauth_token("expired-complete", "refresh-token", -10),
        vec![
            oauth_token("startup-refresh", "refresh-token", -10),
            oauth_token("cycle-refresh-1", "refresh-token", -10),
            oauth_token("cycle-refresh-2", "refresh-token", -10),
        ],
    );
    let sync = RecordingSync::default();
    let h = make_harness_with_config_smtp_oauth(
        no_imap(),
        sync.syncer(),
        MockSmtp::ok().submitter(),
        oauth,
        fast_worker_config(),
    )
    .await;
    let account_id = make_oauth_account(&h, "gmail-worker@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let complete = c
        .request(
            "oauth.google.complete",
            json!({
                "account_id": account_id,
                "client_id": "client",
                "client_secret": "secret",
                "redirect_uri": "http://127.0.0.1/callback",
                "code": "code",
                "state": "state",
                "expected_state": "state",
            }),
        )
        .await
        .unwrap();
    assert!(complete.ok, "{:?}", complete.error);

    let started = c
        .request(
            "account.start_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(started.ok, "{:?}", started.error);

    let secrets = wait_for_recorded_secrets(&sync, 2).await;
    let stopped = c
        .request(
            "account.stop_sync",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(stopped.ok, "{:?}", stopped.error);

    assert_eq!(
        &secrets[..2],
        ["cycle-refresh-1".to_string(), "cycle-refresh-2".to_string()]
    );
}

#[tokio::test]
async fn account_start_sync_missing_secret_returns_missing_secret() {
    let h = make_harness_with_sync_config(
        ScriptedSync::new(1, 1, vec![]).syncer(),
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
        ScriptedSync::new(1, 1, vec![]).syncer(),
        fast_worker_config(),
    )
    .await;
    let id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let missing_account = c
        .request(
            "account.start_sync",
            json!({"account_id": AccountId::new(), "folder_name": "INBOX"}),
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
async fn account_sync_folder_oauth_account_refreshes_and_uses_bearer_token() {
    let oauth = google_oauth_with_exchange_and_refresh(
        oauth_token("expired-access", "refresh-token", -10),
        oauth_token("fresh-access", "refresh-token", 3600),
    );
    let sync =
        ScriptedSync::with_credential(1, 1, vec![], CredentialKind::OAuth2Bearer, "fresh-access");
    let h = make_harness_with_config_smtp_oauth(
        no_imap(),
        sync.syncer(),
        MockSmtp::ok().submitter(),
        oauth,
        WorkerConfig::default(),
    )
    .await;
    let account_id = make_oauth_account(&h, "gmail@example.com").await;
    folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let complete = c
        .request(
            "oauth.google.complete",
            json!({
                "account_id": account_id,
                "client_id": "client",
                "client_secret": "secret",
                "redirect_uri": "http://127.0.0.1/callback",
                "code": "code",
                "state": "state",
                "expected_state": "state",
            }),
        )
        .await
        .unwrap();
    assert!(complete.ok, "{:?}", complete.error);

    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["inserted"], json!(0));

    let stored = google::load_stored_oauth(h.secrets.as_ref(), account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.token.access_token, "fresh-access");
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
    let h = make_harness_with_sync(ScriptedSync::new(1, 7, msgs).syncer()).await;
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
async fn attachment_list_preview_export_round_trip_over_socket() {
    let h = make_harness().await;
    let account_id = make_account(&h, "attachments@example.com").await;
    let folder = folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id,
            name: "INBOX".into(),
            delimiter: "/".into(),
            role: FolderRole::Inbox,
            selectable: true,
        },
    )
    .await
    .unwrap();
    let thread = threads::create(&h.pool, account_id, None, Some("attached"))
        .await
        .unwrap();
    let msg = messages::create(
        &h.pool,
        &messages::NewMessage {
            account_id,
            folder_id: folder.id,
            thread_id: Some(thread.id),
            uid: 99,
            message_id_header: Some("attachment-list@example.com".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "alice@example.com".into(),
            to_addrs: json!(["bob@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("attached".into()),
            snippet: Some("body".into()),
            text_body: Some("body".into()),
            html_body: None,
            raw_size: 12,
            flags: json!([]),
            internal_date: chrono::Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();
    let source = h._db_dir.path().join("source-note.txt");
    tokio::fs::write(&source, b"hello safe preview")
        .await
        .unwrap();
    let attachment = db_attachments::create(
        &h.pool,
        &db_attachments::NewAttachment {
            message_id: msg.id,
            filename: "source-note.txt".into(),
            content_type: "text/plain".into(),
            content_id: None,
            size_bytes: 18,
            disposition: AttachmentDisposition::Attachment,
            storage_path: source.display().to_string(),
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let listed = c
        .request("attachment.list", json!({"message_id": msg.id}))
        .await
        .unwrap();
    assert!(listed.ok, "{:?}", listed.error);
    assert_eq!(listed.data.as_array().unwrap().len(), 1);
    assert_eq!(listed.data[0]["id"], attachment.id.to_string());

    let preview = c
        .request("attachment.preview", json!({"id": attachment.id}))
        .await
        .unwrap();
    assert!(preview.ok, "{:?}", preview.error);
    assert_eq!(preview.data["inline_text"], "hello safe preview");
    assert_eq!(preview.data["truncated"], false);

    let exported_path = h._db_dir.path().join("exported-note.txt");
    let exported = c
        .request(
            "attachment.export",
            json!({"id": attachment.id, "destination_path": exported_path}),
        )
        .await
        .unwrap();
    assert!(exported.ok, "{:?}", exported.error);
    assert_eq!(
        tokio::fs::read(&exported_path).await.unwrap(),
        b"hello safe preview"
    );

    let overwrite = c
        .request(
            "attachment.export",
            json!({"id": attachment.id, "destination_path": exported_path}),
        )
        .await
        .unwrap();
    assert!(!overwrite.ok);
    assert_eq!(
        overwrite.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn account_sync_folder_persists_attachment_bytes_for_safe_preview() {
    let attachment_text = "sync attachment preview text";
    let msgs = vec![FetchedMessage {
        uid: 55,
        flags: vec![],
        internal_date: Some(chrono::Utc::now()),
        raw: rfc822_with_text_attachment("sync-attachment@x", "sync.txt", attachment_text),
    }];
    let h = make_harness_with_sync(ScriptedSync::new(1, 56, msgs).syncer()).await;
    let account_id = setup_account_with_secret(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "account.sync_folder",
            json!({"account_id": account_id, "folder_name": "INBOX"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["inserted"], json!(1));

    let folder = folders::get_by_name(&h.pool, account_id, "INBOX")
        .await
        .unwrap()
        .unwrap();
    let rows = messages::list_by_folder(&h.pool, folder.id, 100, 0)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    let listed = c
        .request("attachment.list", json!({"message_id": rows[0].id}))
        .await
        .unwrap();
    assert!(listed.ok, "{:?}", listed.error);
    let attachment_id = listed.data[0]["id"].as_str().unwrap().to_string();
    assert_eq!(listed.data[0]["filename"], "sync.txt");
    let storage_path = listed.data[0]["storage_path"].as_str().unwrap();
    assert_eq!(
        tokio::fs::read_to_string(storage_path).await.unwrap(),
        attachment_text
    );

    let preview = c
        .request("attachment.preview", json!({"id": attachment_id}))
        .await
        .unwrap();
    assert!(preview.ok, "{:?}", preview.error);
    assert_eq!(preview.data["inline_text"], attachment_text);
}

#[tokio::test]
async fn account_sync_folder_skips_already_present_uids() {
    let msgs = vec![FetchedMessage {
        uid: 1,
        flags: vec![],
        internal_date: Some(chrono::Utc::now()),
        raw: rfc822("dup@x", None, "Hi", "dup"),
    }];
    let scripted = ScriptedSync::new(1, 2, msgs);
    let h = make_harness_with_sync(scripted.syncer()).await;
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
    let scripted = ScriptedSync::new(
        1,
        2,
        vec![FetchedMessage {
            uid: 1,
            flags: vec![],
            internal_date: Some(chrono::Utc::now()),
            raw: rfc822("first@x", None, "first", "body1"),
        }],
    );
    let h = make_harness_with_sync(scripted.syncer()).await;
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
    let h = make_harness_with_sync(ScriptedSync::new(1, 1, vec![]).syncer()).await;
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
    let h = make_harness_with_sync(ScriptedSync::new(1, 1, vec![]).syncer()).await;
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
    let h = make_harness_with_sync(ScriptedSync::new(1, 1, vec![]).syncer()).await;
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
    let h = make_harness_with_sync(ScriptedSync::new(1, 1, vec![]).syncer()).await;
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

/// Seed an account with INBOX/Archive/Trash folders and one message in INBOX.
/// Returns (account_id, inbox_id, archive_id, trash_id, message_id).
async fn seed_account_with_message(
    h: &Harness,
) -> (AccountId, FolderId, FolderId, FolderId, MessageId) {
    let acc = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: format!("a-{}@x.com", uuid::Uuid::new_v4()),
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
    let inbox = folders::upsert(
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
    let archive = folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: acc.id,
            name: "Archive".into(),
            delimiter: "/".into(),
            role: FolderRole::Archive,
            selectable: true,
        },
    )
    .await
    .unwrap();
    let trash = folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id: acc.id,
            name: "Trash".into(),
            delimiter: "/".into(),
            role: FolderRole::Trash,
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
            folder_id: inbox.id,
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
    (acc.id, inbox.id, archive.id, trash.id, msg.id)
}

#[tokio::test]
async fn message_archive_moves_row_publishes_event_and_audits() {
    let h = make_harness().await;
    let (_, inbox_id, archive_id, _, msg_id) = seed_account_with_message(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let sub = c.subscribe(Topic::MailUpdated).await.unwrap();
    assert!(sub > 0);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resp = c
        .request("message.archive", json!({"id": msg_id.to_string()}))
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["folder_id"], archive_id.to_string());

    let event = timeout(Duration::from_secs(2), c.next_event())
        .await
        .expect("expected mail.updated event")
        .unwrap();
    assert_eq!(event.topic, "mail.updated");
    assert_eq!(event.data["message_id"], msg_id.to_string());
    assert_eq!(event.data["folder_id"], archive_id.to_string());
    assert_eq!(event.data["from_folder_id"], inbox_id.to_string());

    let row = messages::get(&h.pool, msg_id).await.unwrap().unwrap();
    assert_eq!(row.folder_id, archive_id);

    let audit = c
        .request("audit.list_recent", json!({"limit": 10}))
        .await
        .unwrap();
    assert!(audit.ok);
    assert!(
        audit
            .data
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "message.archive" && e["target"] == msg_id.to_string()),
        "expected audit entry for message.archive"
    );
}

#[tokio::test]
async fn message_delete_moves_row_to_trash_and_publishes_event() {
    let h = make_harness().await;
    let (_, inbox_id, _, trash_id, msg_id) = seed_account_with_message(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let sub = c.subscribe(Topic::MailUpdated).await.unwrap();
    assert!(sub > 0);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resp = c
        .request("message.delete", json!({"id": msg_id.to_string()}))
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["folder_id"], trash_id.to_string());

    let event = timeout(Duration::from_secs(2), c.next_event())
        .await
        .expect("expected mail.updated event")
        .unwrap();
    assert_eq!(event.data["message_id"], msg_id.to_string());
    assert_eq!(event.data["folder_id"], trash_id.to_string());
    assert_eq!(event.data["from_folder_id"], inbox_id.to_string());

    let row = messages::get(&h.pool, msg_id).await.unwrap().unwrap();
    assert_eq!(row.folder_id, trash_id);
}

#[tokio::test]
async fn message_move_to_named_folder_moves_row_and_publishes_event() {
    let h = make_harness().await;
    let (account_id, inbox_id, _, _, msg_id) = seed_account_with_message(&h).await;
    let custom = folders::upsert(
        &h.pool,
        &folders::NewFolder {
            account_id,
            name: "Receipts".into(),
            delimiter: "/".into(),
            role: FolderRole::Custom,
            selectable: true,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let sub = c.subscribe(Topic::MailUpdated).await.unwrap();
    assert!(sub > 0);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let resp = c
        .request(
            "message.move",
            json!({"id": msg_id.to_string(), "folder_name": "Receipts"}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["folder_id"], custom.id.to_string());

    let event = timeout(Duration::from_secs(2), c.next_event())
        .await
        .expect("expected mail.updated event")
        .unwrap();
    assert_eq!(event.data["message_id"], msg_id.to_string());
    assert_eq!(event.data["folder_id"], custom.id.to_string());
    assert_eq!(event.data["from_folder_id"], inbox_id.to_string());

    let row = messages::get(&h.pool, msg_id).await.unwrap().unwrap();
    assert_eq!(row.folder_id, custom.id);
}

#[tokio::test]
async fn message_archive_unknown_id_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "message.archive",
            json!({"id": MessageId::new().to_string()}),
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
async fn message_archive_without_archive_folder_returns_bad_args() {
    let h = make_harness().await;
    // Seed an account with INBOX only — no Archive role.
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
    let inbox = folders::upsert(
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
            folder_id: inbox.id,
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
    let resp = c
        .request("message.archive", json!({"id": msg.id.to_string()}))
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn message_move_unknown_folder_returns_bad_args() {
    let h = make_harness().await;
    let (_, _, _, _, msg_id) = seed_account_with_message(&h).await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "message.move",
            json!({"id": msg_id.to_string(), "folder_name": "DoesNotExist"}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

// `:archive` must drive the same daemon op + hub event as the `e`
// keybinding. Both code paths route through `MailboxClient::archive_message`,
// so we drive that path here against a real socket and assert the same
// DB + hub state as `message_archive_moves_row_publishes_event_and_audits`.
#[tokio::test]
async fn tui_command_archive_matches_archive_keybinding_via_daemon() {
    use postblox::tui::command::{parse_command, Command};
    use postblox::tui::ipc::MailboxClient;

    let h = make_harness().await;
    let (_, inbox_id, archive_id, _, msg_id) = seed_account_with_message(&h).await;

    // Command-bar parser must yield the same Command::Archive that the
    // `e` key dispatches.
    assert_eq!(parse_command("archive").unwrap(), Command::Archive);

    // Subscribe through the high-level TUI client so we exercise the
    // same client wire shape the live TUI uses.
    let mut events = Client::connect(&h.sock).await.unwrap();
    let sub = events.subscribe(Topic::MailUpdated).await.unwrap();
    assert!(sub > 0);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut tui = MailboxClient::connect(&h.sock).await.unwrap();
    tui.archive_message(msg_id).await.unwrap();

    let event = timeout(Duration::from_secs(2), events.next_event())
        .await
        .expect("expected mail.updated event")
        .unwrap();
    assert_eq!(event.topic, "mail.updated");
    assert_eq!(event.data["message_id"], msg_id.to_string());
    assert_eq!(event.data["folder_id"], archive_id.to_string());
    assert_eq!(event.data["from_folder_id"], inbox_id.to_string());

    let row = messages::get(&h.pool, msg_id).await.unwrap().unwrap();
    assert_eq!(row.folder_id, archive_id);
}

// FTS5 search end-to-end: seed two accounts each with one message, then
// drive the daemon `search` op through the socket and assert (a)
// unscoped queries return both, (b) `account_id` filters down to one,
// and (c) empty `q` is rejected.
async fn seed_searchable_message(
    h: &Harness,
    email: &str,
    subject: &str,
    body: &str,
) -> (AccountId, MessageId) {
    let acc = accounts::create(
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
    let thread = threads::create(&h.pool, acc.id, None, Some(subject))
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
            from_addr: "sender@example.com".into(),
            to_addrs: json!([]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some(subject.into()),
            snippet: Some(body.into()),
            text_body: Some(body.into()),
            html_body: None,
            raw_size: body.len() as i64,
            flags: json!([]),
            internal_date: chrono::Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();
    (acc.id, msg.id)
}

#[tokio::test]
async fn search_op_returns_messages_across_accounts_when_unscoped() {
    let h = make_harness().await;
    let (_acc_a, msg_a) =
        seed_searchable_message(&h, "a@x.com", "Quarterly review", "review notes").await;
    let (_acc_b, msg_b) =
        seed_searchable_message(&h, "b@x.com", "Quarterly meeting", "agenda").await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request("search", json!({"q": "quarterly"}))
        .await
        .unwrap();
    assert!(resp.ok, "search failed: {:?}", resp.error);
    let rows = resp.data.as_array().expect("array");
    let ids: Vec<String> = rows
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect();
    assert!(ids.contains(&msg_a.to_string()));
    assert!(ids.contains(&msg_b.to_string()));
}

#[tokio::test]
async fn search_op_filters_by_account_id_when_scoped() {
    let h = make_harness().await;
    let (acc_a, msg_a) =
        seed_searchable_message(&h, "a@x.com", "Quarterly review", "review notes").await;
    let (_acc_b, msg_b) =
        seed_searchable_message(&h, "b@x.com", "Quarterly meeting", "agenda").await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "search",
            json!({"q": "quarterly", "account_id": acc_a.to_string()}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "search failed: {:?}", resp.error);
    let ids: Vec<String> = resp
        .data
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect();
    assert!(ids.contains(&msg_a.to_string()));
    assert!(!ids.contains(&msg_b.to_string()));
}

#[tokio::test]
async fn message_summary_ops_omit_bodies_but_message_get_includes_them() {
    let h = make_harness().await;
    let acc = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: "summary@x.com".into(),
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
    let thread = threads::create(&h.pool, acc.id, None, Some("Unique Summary Marker"))
        .await
        .unwrap();
    let message = messages::create(
        &h.pool,
        &messages::NewMessage {
            account_id: acc.id,
            folder_id: folder.id,
            thread_id: Some(thread.id),
            uid: 77,
            message_id_header: Some("<summary@x>".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: json!(["reader@example.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("Unique Summary Marker".into()),
            snippet: Some("preview only".into()),
            text_body: Some("full text body must stay detail-only".into()),
            html_body: Some("<p>full html body must stay detail-only</p>".into()),
            raw_size: 2048,
            flags: json!(["\\Seen"]),
            internal_date: chrono::Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let list = c
        .request(
            "message.list_by_folder",
            json!({"folder_id": folder.id.to_string(), "limit": 10}),
        )
        .await
        .unwrap();
    assert!(list.ok, "{:?}", list.error);
    let list_row = list.data.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(list_row["id"], message.id.to_string());
    assert_eq!(list_row["subject"], "Unique Summary Marker");
    assert!(!list_row.contains_key("text_body"));
    assert!(!list_row.contains_key("html_body"));

    let thread_list = c
        .request(
            "message.list_by_thread",
            json!({"thread_id": thread.id.to_string()}),
        )
        .await
        .unwrap();
    assert!(thread_list.ok, "{:?}", thread_list.error);
    let thread_row = thread_list.data.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(thread_row["id"], message.id.to_string());
    assert!(!thread_row.contains_key("text_body"));
    assert!(!thread_row.contains_key("html_body"));

    let search = c.request("search", json!({"q": "unique"})).await.unwrap();
    assert!(search.ok, "{:?}", search.error);
    let search_row = search
        .data
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == message.id.to_string())
        .unwrap()
        .as_object()
        .unwrap();
    assert!(!search_row.contains_key("text_body"));
    assert!(!search_row.contains_key("html_body"));

    let detail = c
        .request("message.get", json!({"id": message.id.to_string()}))
        .await
        .unwrap();
    assert!(detail.ok, "{:?}", detail.error);
    assert_eq!(
        detail.data["text_body"],
        "full text body must stay detail-only"
    );
    assert_eq!(
        detail.data["html_body"],
        "<p>full html body must stay detail-only</p>"
    );
}

#[tokio::test]
async fn search_op_empty_query_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let resp = c.request("search", json!({"q": ""})).await.unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
}

#[tokio::test]
async fn search_op_clamps_limit_to_two_hundred() {
    let h = make_harness().await;
    seed_searchable_message(&h, "a@x.com", "review", "lorem").await;

    let mut c = Client::connect(&h.sock).await.unwrap();
    // limit=9999 must not crash or error — the dispatcher silently
    // clamps to 200 so the IPC frame stays bounded.
    let resp = c
        .request("search", json!({"q": "review", "limit": 9999}))
        .await
        .unwrap();
    assert!(resp.ok, "search failed: {:?}", resp.error);
}

// Subscribe → publish round-trip is already covered by
// `subscription_delivers_published_event_via_hub` above; the test below
// exercises the TUI reducer that sits *behind* that subscribe path so we
// know the wire shape we publish today is the one the TUI consumes.
#[tokio::test]
async fn tui_reducer_creates_toast_for_mail_new_event() {
    use postblox::ipc::Event as IpcEvent;
    use postblox::tui::app::{AccountItem, AppState, FolderItem, ToastKind};
    use postblox::tui::on_daemon_event;
    let account_id = AccountId::new();
    let folder_id = FolderId::new();
    let mut app = AppState::default();
    app.apply_accounts(vec![AccountItem {
        id: account_id,
        label: "Work".into(),
        email: "work@example.com".into(),
        status: "idle".into(),
    }]);
    app.apply_folders(vec![FolderItem {
        id: folder_id,
        name: "INBOX".into(),
        role: "inbox".into(),
    }]);

    let event = IpcEvent {
        sub: 1,
        topic: "mail.new".into(),
        data: json!({
            "account_id": account_id.to_string(),
            "folder_id": folder_id.to_string(),
            "thread_id": ThreadId::new().to_string(),
            "message_id": MessageId::new().to_string(),
            "uid": 42,
        }),
    };

    on_daemon_event(&mut app, &event);

    assert_eq!(app.toasts.len(), 1);
    let toast = app.toasts.back().unwrap();
    assert_eq!(toast.kind, ToastKind::Info);
    assert!(toast.text.contains("INBOX"));
    assert!(toast.text.contains("Work"));
}

#[tokio::test]
async fn draft_create_with_attachments_persists_rows() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("note.txt");
    tokio::fs::write(&path, b"hello attach").await.unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "with attach",
                "text_body": "body",
                "attachments": [{"path": path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let draft_id: DraftId = created.data["id"].as_str().unwrap().parse().unwrap();

    let rows = postblox::db::draft_attachments::list_for_draft(&h.pool, draft_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].filename, "note.txt");
    assert_eq!(rows[0].content_type, "text/plain");
    assert_eq!(rows[0].size_bytes, b"hello attach".len() as i64);
    let bytes = postblox::db::draft_attachments::load_content(&h.pool, rows[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"hello attach");
}

#[tokio::test]
async fn draft_update_replaces_attachments() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.bin");
    tokio::fs::write(&first, b"first").await.unwrap();
    tokio::fs::write(&second, b"second-bytes").await.unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "before",
                "text_body": "body",
                "attachments": [{"path": first.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let draft_id: DraftId = created.data["id"].as_str().unwrap().parse().unwrap();

    // Replace with a different file.
    let updated = c
        .request(
            "draft.update",
            json!({
                "id": draft_id.to_string(),
                "to_addrs": ["bob@x.com"],
                "subject": "after",
                "attachments": [
                    {"path": second.display().to_string(), "filename": "renamed.bin"},
                ],
            }),
        )
        .await
        .unwrap();
    assert!(updated.ok, "{:?}", updated.error);
    let rows = postblox::db::draft_attachments::list_for_draft(&h.pool, draft_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].filename, "renamed.bin");
    assert_eq!(rows[0].size_bytes, b"second-bytes".len() as i64);

    // Omitting `attachments` leaves rows alone.
    let touched = c
        .request(
            "draft.update",
            json!({
                "id": draft_id.to_string(),
                "to_addrs": ["bob@x.com"],
                "subject": "subject only",
            }),
        )
        .await
        .unwrap();
    assert!(touched.ok, "{:?}", touched.error);
    let rows_after = postblox::db::draft_attachments::list_for_draft(&h.pool, draft_id)
        .await
        .unwrap();
    assert_eq!(rows_after.len(), 1);
    assert_eq!(rows_after[0].id, rows[0].id);

    // Empty array clears attachments.
    let cleared = c
        .request(
            "draft.update",
            json!({
                "id": draft_id.to_string(),
                "to_addrs": ["bob@x.com"],
                "subject": "cleared",
                "attachments": [],
            }),
        )
        .await
        .unwrap();
    assert!(cleared.ok, "{:?}", cleared.error);
    assert!(
        postblox::db::draft_attachments::list_for_draft(&h.pool, draft_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn draft_create_attachment_over_limit_returns_attachment_too_large() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    let oversize = postblox::db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES as usize + 1;
    tokio::fs::write(&path, vec![0u8; oversize]).await.unwrap();

    let resp = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "too big",
                "text_body": "body",
                "attachments": [{"path": path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("attachment_too_large")
    );

    // Draft was rolled back on attachment failure.
    let account_id: AccountId = acc_id.parse().unwrap();
    let drafts = postblox::db::drafts::list_by_account(&h.pool, account_id)
        .await
        .unwrap();
    assert!(drafts.is_empty(), "expected rollback, got {drafts:?}");
}

#[tokio::test]
async fn draft_create_attachment_missing_path_returns_bad_args() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap();

    let resp = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "x",
                "text_body": "y",
                "attachments": [{"path": "/nonexistent/postblox-test.bin"}],
            }),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );
    let account_id: AccountId = acc_id.parse().unwrap();
    let drafts = postblox::db::drafts::list_by_account(&h.pool, account_id)
        .await
        .unwrap();
    assert!(drafts.is_empty(), "expected rollback, got {drafts:?}");
}

#[tokio::test]
async fn message_send_with_attachments_builds_multipart_mime() {
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
    let account_id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invoice.pdf");
    let body = b"%PDF-1.4 fake bytes";
    tokio::fs::write(&path, body).await.unwrap();

    let draft = c
        .request(
            "draft.create",
            json!({
                "account_id": account_id,
                "to_addrs": ["to@example.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "with file",
                "text_body": "see attached",
                "html_body": null,
                "attachments": [{"path": path.display().to_string()}],
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

    let calls = smtp.calls();
    assert_eq!(calls.len(), 1);
    let mime = String::from_utf8(calls[0].mime.clone()).unwrap();
    assert!(
        mime.contains("Content-Type: multipart/mixed"),
        "missing multipart/mixed: {mime}"
    );
    assert!(mime.contains("Content-Type: application/pdf; name=\"invoice.pdf\""));
    assert!(
        mime.contains("Content-Disposition: attachment; filename=\"invoice.pdf\""),
        "filename missing: {mime}"
    );
    assert!(mime.contains("Content-Transfer-Encoding: base64"));
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(body);
    let stripped: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let mime_clean: String = mime.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        mime_clean.contains(&stripped),
        "base64 body not present in MIME"
    );

    let audit = c
        .request("audit.list_recent", json!({"limit": 5}))
        .await
        .unwrap();
    assert!(audit.ok);
    let entries = audit.data.as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| { e["action"] == "message.send" && e["details"]["attachment_count"] == 1 }),
        "expected attachment_count in audit details: {entries:?}"
    );
}

// -- Slice 7: reply / reply-all / forward ----------------------------------

async fn insert_reply_seed(h: &Harness) -> (AccountId, FolderId, MessageId) {
    let acc = accounts::create(
        &h.pool,
        &accounts::NewAccount {
            email: "me@example.com".into(),
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
    let inbox = folders::upsert(
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
    let thread = threads::create(&h.pool, acc.id, None, Some("Original"))
        .await
        .unwrap();
    let msg = messages::create(
        &h.pool,
        &messages::NewMessage {
            account_id: acc.id,
            folder_id: inbox.id,
            thread_id: Some(thread.id),
            uid: 7,
            message_id_header: Some("orig@example.com".into()),
            in_reply_to: None,
            references_header: None,
            from_addr: "alice@example.com".into(),
            to_addrs: json!(["me@example.com", "bob@example.com"]),
            cc_addrs: json!(["carol@example.com"]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some("Original".into()),
            snippet: Some("hi".into()),
            text_body: Some("Original body line one\nOriginal body line two".into()),
            html_body: None,
            raw_size: 32,
            flags: json!([]),
            internal_date: chrono::Utc::now(),
            sent_at: None,
        },
    )
    .await
    .unwrap();
    (acc.id, inbox.id, msg.id)
}

#[tokio::test]
async fn message_prepare_reply_returns_threading_headers_and_quoted_body() {
    let h = make_harness().await;
    let (account_id, _folder_id, message_id) = insert_reply_seed(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let resp = c
        .request(
            "message.prepare_reply",
            json!({"message_id": message_id, "reply_all": false}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["account_id"], account_id.to_string());
    assert_eq!(resp.data["subject"], "Re: Original");
    assert_eq!(resp.data["in_reply_to"], "<orig@example.com>");
    assert_eq!(resp.data["references"], "<orig@example.com>");
    let to: Vec<String> = serde_json::from_value(resp.data["to"].clone()).unwrap();
    assert_eq!(to, vec!["alice@example.com".to_string()]);
    let cc: Vec<String> = serde_json::from_value(resp.data["cc"].clone()).unwrap();
    assert!(cc.is_empty());
    let body = resp.data["quoted_body"].as_str().unwrap();
    assert!(body.contains("alice@example.com wrote:"));
    assert!(body.contains("> Original body line one"));
}

#[tokio::test]
async fn message_prepare_reply_all_includes_others_and_drops_self() {
    let h = make_harness().await;
    let (_account_id, _folder_id, message_id) = insert_reply_seed(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let resp = c
        .request(
            "message.prepare_reply",
            json!({"message_id": message_id, "reply_all": true}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    let to: Vec<String> = serde_json::from_value(resp.data["to"].clone()).unwrap();
    assert_eq!(to, vec!["alice@example.com".to_string()]);
    let cc: Vec<String> = serde_json::from_value(resp.data["cc"].clone()).unwrap();
    // me@example.com filtered, alice already in To, others kept.
    assert!(cc.contains(&"bob@example.com".to_string()));
    assert!(cc.contains(&"carol@example.com".to_string()));
    assert!(!cc.iter().any(|s| s.eq_ignore_ascii_case("me@example.com")));
}

#[tokio::test]
async fn message_prepare_forward_returns_subject_body_and_attachment_manifest() {
    let h = make_harness().await;
    let (account_id, _folder_id, message_id) = insert_reply_seed(&h).await;

    // Attach one file.
    let path = h._db_dir.path().join("forward.txt");
    tokio::fs::write(&path, b"forward attachment bytes")
        .await
        .unwrap();
    let attachment = db_attachments::create(
        &h.pool,
        &db_attachments::NewAttachment {
            message_id,
            filename: "forward.txt".into(),
            content_type: "text/plain".into(),
            content_id: None,
            size_bytes: 24,
            disposition: AttachmentDisposition::Attachment,
            storage_path: path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request("message.prepare_forward", json!({"message_id": message_id}))
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["account_id"], account_id.to_string());
    assert_eq!(resp.data["subject"], "Fwd: Original");
    let body = resp.data["forwarded_body"].as_str().unwrap();
    assert!(body.contains("---------- Forwarded message ----------"));
    assert!(body.contains("From: alice@example.com"));
    let manifest = resp.data["forwarded_attachments"].as_array().unwrap();
    assert_eq!(manifest.len(), 1);
    assert_eq!(manifest[0]["attachment_id"], attachment.id.to_string());
    assert_eq!(manifest[0]["filename"], "forward.txt");
    assert_eq!(manifest[0]["size_bytes"], 24);
}

#[tokio::test]
async fn attachment_fetch_for_forward_returns_cached_bytes_when_available() {
    let h = make_harness().await;
    let (_account_id, _folder_id, message_id) = insert_reply_seed(&h).await;
    let path = h._db_dir.path().join("cached.txt");
    let payload = b"cached forward bytes";
    tokio::fs::write(&path, payload).await.unwrap();
    let attachment = db_attachments::create(
        &h.pool,
        &db_attachments::NewAttachment {
            message_id,
            filename: "cached.txt".into(),
            content_type: "text/plain".into(),
            content_id: None,
            size_bytes: payload.len() as i64,
            disposition: AttachmentDisposition::Attachment,
            storage_path: path.display().to_string(),
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "attachment.fetch_for_forward",
            json!({"attachment_id": attachment.id}),
        )
        .await
        .unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.data["filename"], "cached.txt");
    assert_eq!(resp.data["source"], "cache");
    use base64::Engine;
    let encoded = resp.data["content_base64"].as_str().unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(decoded, payload);
}

#[tokio::test]
async fn attachment_fetch_for_forward_without_cache_or_creds_returns_unavailable_offline() {
    let h = make_harness().await;
    let (_account_id, _folder_id, message_id) = insert_reply_seed(&h).await;
    // Reference a path that does not exist on disk.
    let attachment = db_attachments::create(
        &h.pool,
        &db_attachments::NewAttachment {
            message_id,
            filename: "missing.bin".into(),
            content_type: "application/octet-stream".into(),
            content_id: None,
            size_bytes: 0,
            disposition: AttachmentDisposition::Attachment,
            storage_path: h
                ._db_dir
                .path()
                .join("does-not-exist.bin")
                .display()
                .to_string(),
        },
    )
    .await
    .unwrap();

    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request(
            "attachment.fetch_for_forward",
            json!({"attachment_id": attachment.id}),
        )
        .await
        .unwrap();
    assert!(!resp.ok);
    assert_eq!(
        resp.error.as_ref().map(|e| e.code.as_str()),
        Some("unavailable_offline"),
    );
}

#[tokio::test]
async fn message_send_with_reply_headers_emits_in_reply_to_and_references() {
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
    let account_id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let draft = c
        .request(
            "draft.create",
            json!({
                "account_id": account_id,
                "to_addrs": ["alice@example.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "Re: Hi",
                "text_body": "thanks",
                "in_reply_to": "<orig@example.com>",
                "references_header": "<root@example.com> <orig@example.com>",
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

    let calls = smtp.calls();
    assert_eq!(calls.len(), 1);
    let mime = String::from_utf8(calls[0].mime.clone()).unwrap();
    assert!(mime.contains("In-Reply-To: <orig@example.com>\r\n"));
    assert!(mime.contains("References: <root@example.com> <orig@example.com>\r\n"));
}

// -- Slice 8: drafts list / get / delete -------------------------------------

#[tokio::test]
async fn draft_list_orders_by_updated_at_desc() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap().to_string();

    let first = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "first",
                "text_body": "1",
                "html_body": null,
                "in_reply_to_msg": null,
            }),
        )
        .await
        .unwrap();
    assert!(first.ok, "{:?}", first.error);
    // Tiny sleep so updated_at strictly differs (millisecond resolution).
    tokio::time::sleep(Duration::from_millis(15)).await;
    let second = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["carol@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "second",
                "text_body": "2",
                "html_body": null,
                "in_reply_to_msg": null,
            }),
        )
        .await
        .unwrap();
    assert!(second.ok);

    let listed = c
        .request("draft.list", json!({ "account_id": acc_id }))
        .await
        .unwrap();
    assert!(listed.ok);
    let rows = listed.data.as_array().expect("array");
    assert_eq!(rows.len(), 2);
    // Newest first.
    assert_eq!(rows[0]["subject"], "second");
    assert_eq!(rows[1]["subject"], "first");
}

#[tokio::test]
async fn draft_get_round_trip_returns_draft_and_attachment_bytes() {
    use base64::Engine;
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap().to_string();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.txt");
    tokio::fs::write(&path, b"draft notes").await.unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "with attachment",
                "text_body": "see file",
                "html_body": null,
                "in_reply_to": "<orig@x.com>",
                "references_header": "<root@x.com> <orig@x.com>",
                "attachments": [{"path": path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let draft_id = created.data["id"].as_str().unwrap().to_string();

    let got = c
        .request("draft.get", json!({ "id": draft_id }))
        .await
        .unwrap();
    assert!(got.ok, "{:?}", got.error);
    let draft = &got.data["draft"];
    assert_eq!(draft["subject"], "with attachment");
    assert_eq!(draft["in_reply_to"], "<orig@x.com>");
    assert_eq!(draft["references_header"], "<root@x.com> <orig@x.com>");

    let attachments = got.data["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["filename"], "notes.txt");
    let encoded = attachments[0]["content_base64"].as_str().unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(decoded, b"draft notes");
}

#[tokio::test]
async fn draft_update_invalid_attachment_path_leaves_draft_and_attachments_unchanged() {
    use base64::Engine;
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap().to_string();

    let dir = tempfile::tempdir().unwrap();
    let original_path = dir.path().join("original.txt");
    tokio::fs::write(&original_path, b"original attachment")
        .await
        .unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "original",
                "text_body": "original body",
                "html_body": null,
                "attachments": [{"path": original_path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(created.ok, "{:?}", created.error);
    let draft_id = created.data["id"].as_str().unwrap().to_string();

    let missing_path = dir.path().join("missing.txt");
    let updated = c
        .request(
            "draft.update",
            json!({
                "id": draft_id,
                "to_addrs": ["alice@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "edited",
                "text_body": "edited body",
                "html_body": null,
                "attachments": [{"path": missing_path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(!updated.ok);
    assert_eq!(
        updated.error.as_ref().map(|e| e.code.as_str()),
        Some("bad_args")
    );

    let got = c
        .request("draft.get", json!({ "id": draft_id }))
        .await
        .unwrap();
    assert!(got.ok, "{:?}", got.error);
    let draft = &got.data["draft"];
    assert_eq!(draft["subject"], "original");
    assert_eq!(draft["text_body"], "original body");
    assert_eq!(draft["to_addrs"], json!(["bob@x.com"]));

    let attachments = got.data["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["filename"], "original.txt");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(attachments[0]["content_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, b"original attachment");
}

#[tokio::test]
async fn draft_create_oversized_sparse_attachment_rejects_before_reading_full_file() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap().to_string();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("too-large.bin");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len((postblox::db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES as u64) + 1)
        .unwrap();
    drop(file);

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "too large",
                "text_body": "body",
                "html_body": null,
                "attachments": [{"path": path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    assert!(!created.ok);
    assert_eq!(
        created.error.as_ref().map(|e| e.code.as_str()),
        Some("attachment_too_large")
    );

    let listed = c
        .request("draft.list", json!({ "account_id": acc_id }))
        .await
        .unwrap();
    assert!(listed.ok, "{:?}", listed.error);
    assert!(listed.data.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn draft_get_returns_null_for_missing_draft() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();
    let resp = c
        .request("draft.get", json!({ "id": DraftId::new() }))
        .await
        .unwrap();
    assert!(resp.ok);
    assert!(resp.data.is_null());
}

#[tokio::test]
async fn draft_delete_cascades_attachments() {
    let h = make_harness().await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let acc = c
        .request("account.create", account_args("a@x.com"))
        .await
        .unwrap();
    let acc_id = acc.data["id"].as_str().unwrap().to_string();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blob.bin");
    tokio::fs::write(&path, b"xxxxx").await.unwrap();

    let created = c
        .request(
            "draft.create",
            json!({
                "account_id": acc_id,
                "to_addrs": ["bob@x.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "going away",
                "text_body": "soon",
                "html_body": null,
                "attachments": [{"path": path.display().to_string()}],
            }),
        )
        .await
        .unwrap();
    let draft_id = created.data["id"].as_str().unwrap().to_string();

    let deleted = c
        .request("draft.delete", json!({ "id": draft_id }))
        .await
        .unwrap();
    assert!(deleted.ok);
    assert_eq!(deleted.data["removed"], true);

    // After delete the draft is gone (`draft.get` returns null) and
    // listing the account shows zero drafts.
    let got = c
        .request("draft.get", json!({ "id": draft_id }))
        .await
        .unwrap();
    assert!(got.ok);
    assert!(got.data.is_null());

    let listed = c
        .request("draft.list", json!({ "account_id": acc_id }))
        .await
        .unwrap();
    assert!(listed.ok);
    let rows = listed.data.as_array().unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn message_send_removes_draft_after_smtp_accepts() {
    let smtp = MockSmtp::ok();
    let h = make_harness_with_smtp(smtp.submitter()).await;
    let account_id = setup_account_with_secret(&h).await;
    let mut c = Client::connect(&h.sock).await.unwrap();

    let draft = c
        .request(
            "draft.create",
            json!({
                "account_id": account_id,
                "to_addrs": ["to@example.com"],
                "cc_addrs": [],
                "bcc_addrs": [],
                "subject": "send me",
                "text_body": "body",
                "html_body": null,
                "in_reply_to_msg": null,
            }),
        )
        .await
        .unwrap();
    let draft_id = draft.data["id"].as_str().unwrap().to_string();

    let sent = c
        .request(
            "message.send",
            json!({"account_id": account_id, "draft_id": draft_id}),
        )
        .await
        .unwrap();
    assert!(sent.ok);

    // Draft is cleaned up.
    let got = c
        .request("draft.get", json!({ "id": draft_id }))
        .await
        .unwrap();
    assert!(got.ok);
    assert!(got.data.is_null());
}
