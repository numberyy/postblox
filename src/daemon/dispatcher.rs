//! `DaemonDispatcher` — the concrete `ipc::Dispatcher` impl.
//!
//! Maps wire op names to `db::*` calls and publishes events on the
//! [`Hub`] for write ops. No IMAP/SMTP yet — that wires in R3b.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::db;
use crate::imap::{self, ImapAuth, ImapSync};
use crate::ipc::{Dispatcher, Hub, RpcError, Topic};
use crate::models::FolderRole;
use crate::secrets::SecretStore;
use crate::sync;

#[derive(Clone)]
pub struct DaemonDispatcher {
    pool: SqlitePool,
    hub: Arc<Hub>,
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    secrets: Arc<dyn SecretStore>,
    worker_manager: Arc<sync::WorkerManager>,
}

impl DaemonDispatcher {
    /// Production constructor: TLS-backed IMAP via rustls.
    pub fn new(pool: SqlitePool, hub: Arc<Hub>, secrets: Arc<dyn SecretStore>) -> Self {
        let imap = imap::default_auth().expect("rustls platform verifier init");
        let imap_sync = imap::default_sync().expect("rustls platform verifier init");
        Self::with_imap(pool, hub, imap, imap_sync, secrets)
    }

    /// Test/customisation constructor: bring your own IMAP impls.
    pub fn with_imap(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        let worker_manager = Arc::new(sync::WorkerManager::new(
            pool.clone(),
            hub.clone(),
            imap_sync.clone(),
        ));
        Self::with_imap_and_manager(pool, hub, imap, imap_sync, secrets, worker_manager)
    }

    pub fn with_imap_and_sync_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        worker_config: sync::WorkerConfig,
    ) -> Self {
        let worker_manager = Arc::new(sync::WorkerManager::with_config(
            pool.clone(),
            hub.clone(),
            imap_sync.clone(),
            worker_config,
        ));
        Self::with_imap_and_manager(pool, hub, imap, imap_sync, secrets, worker_manager)
    }

    pub fn with_imap_and_manager(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        worker_manager: Arc<sync::WorkerManager>,
    ) -> Self {
        Self {
            pool,
            hub,
            imap,
            imap_sync,
            secrets,
            worker_manager,
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl Dispatcher for DaemonDispatcher {
    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError> {
        match op {
            // -- read ops --
            "account.list" => op_account_list(&self.pool).await,
            "folder.list" => op_folder_list(&self.pool, args).await,
            "thread.list" => op_thread_list(&self.pool, args).await,
            "message.list_by_folder" => op_messages_by_folder(&self.pool, args).await,
            "message.list_by_thread" => op_messages_by_thread(&self.pool, args).await,
            "message.get" => op_message_get(&self.pool, args).await,
            "search" => op_search(&self.pool, args).await,
            "audit.list_recent" => op_audit_list(&self.pool, args).await,

            // -- write ops --
            "account.create" => op_account_create(&self.pool, args).await,
            "account.delete" => op_account_delete(&self.pool, args).await,
            "folder.upsert" => op_folder_upsert(&self.pool, args).await,
            "message.set_flags" => op_message_set_flags(&self.pool, &self.hub, args).await,
            "draft.create" => op_draft_create(&self.pool, args).await,
            "draft.update" => op_draft_update(&self.pool, args).await,
            "draft.delete" => op_draft_delete(&self.pool, args).await,

            // -- network ops --
            "account.test_login" => {
                op_account_test_login(&self.pool, self.imap.as_ref(), args).await
            }
            "account.sync_folder" => {
                op_account_sync_folder(
                    &self.pool,
                    &self.hub,
                    self.imap_sync.as_ref(),
                    self.secrets.as_ref(),
                    args,
                )
                .await
            }
            "account.start_sync" => {
                op_account_start_sync(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.worker_manager.as_ref(),
                    args,
                )
                .await
            }
            "account.stop_sync" => {
                op_account_stop_sync(&self.pool, self.worker_manager.as_ref(), args).await
            }

            // -- secret ops --
            "account.set_secret" => {
                op_account_set_secret(&self.pool, self.secrets.as_ref(), args).await
            }
            "account.delete_secret" => {
                op_account_delete_secret(&self.pool, self.secrets.as_ref(), args).await
            }

            other => Err(RpcError::unknown_op(other)),
        }
    }
}

// ---------- read ops --------------------------------------------------------

async fn op_account_list(pool: &SqlitePool) -> Result<Value, RpcError> {
    encode(db::accounts::list(pool).await, "accounts::list")
}

async fn op_folder_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "account_id")?;
    encode(
        db::folders::list_by_account(pool, id).await,
        "folders::list_by_account",
    )
}

async fn op_thread_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::threads::list_recent(pool, account_id, limit, offset).await,
        "threads::list_recent",
    )
}

async fn op_messages_by_folder(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let folder_id = parse_uuid(&args, "folder_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::messages::list_by_folder(pool, folder_id, limit, offset).await,
        "messages::list_by_folder",
    )
}

async fn op_messages_by_thread(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let thread_id = parse_uuid(&args, "thread_id")?;
    encode(
        db::messages::list_by_thread(pool, thread_id).await,
        "messages::list_by_thread",
    )
}

async fn op_message_get(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    encode(db::messages::get(pool, id).await, "messages::get")
}

async fn op_search(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let q = args
        .get("q")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'q'"))?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::search::search(pool, &db::search::quote_term(q), limit, offset).await,
        "search",
    )
}

async fn op_audit_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::audit::list_recent(pool, limit, offset).await,
        "audit::list_recent",
    )
}

// ---------- write ops -------------------------------------------------------

async fn op_account_create(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let new: db::accounts::NewAccount =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let acc = db::accounts::create(pool, &new)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::create: {e}")))?;
    audit(
        pool,
        "account.create",
        Some(&acc.id.to_string()),
        &json!({}),
    )
    .await;
    encode_one(&acc)
}

async fn op_account_delete(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    let removed = db::accounts::delete(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::delete: {e}")))?;
    audit(pool, "account.delete", Some(&id.to_string()), &json!({})).await;
    Ok(json!({"removed": removed}))
}

async fn op_folder_upsert(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let new: db::folders::NewFolder =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let folder = db::folders::upsert(pool, &new)
        .await
        .map_err(|e| RpcError::internal(format!("folders::upsert: {e}")))?;
    encode_one(&folder)
}

async fn op_message_set_flags(
    pool: &SqlitePool,
    hub: &Hub,
    args: Value,
) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    let flags = args
        .get("flags")
        .cloned()
        .ok_or_else(|| RpcError::bad_args("missing 'flags'"))?;
    db::messages::set_flags(pool, id, &flags)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_flags: {e}")))?;
    audit(
        pool,
        "message.set_flags",
        Some(&id.to_string()),
        &json!({"flags": &flags}),
    )
    .await;
    hub.publish(
        Topic::MailUpdated,
        json!({"message_id": id.to_string(), "flags": flags}),
    )
    .await;
    Ok(json!({"ok": true}))
}

async fn op_draft_create(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let new: db::drafts::NewDraft =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let draft = db::drafts::create(pool, &new)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::create: {e}")))?;
    audit(
        pool,
        "draft.create",
        Some(&draft.id.to_string()),
        &json!({}),
    )
    .await;
    encode_one(&draft)
}

#[derive(Deserialize)]
struct DraftUpdate {
    id: uuid::Uuid,
    #[serde(default = "default_addrs")]
    to_addrs: Value,
    #[serde(default = "default_addrs")]
    cc_addrs: Value,
    #[serde(default = "default_addrs")]
    bcc_addrs: Value,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    text_body: Option<String>,
    #[serde(default)]
    html_body: Option<String>,
}

fn default_addrs() -> Value {
    json!([])
}

async fn op_draft_update(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let upd: DraftUpdate =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let patch = db::drafts::DraftPatch {
        to_addrs: &upd.to_addrs,
        cc_addrs: &upd.cc_addrs,
        bcc_addrs: &upd.bcc_addrs,
        subject: upd.subject.as_deref(),
        text_body: upd.text_body.as_deref(),
        html_body: upd.html_body.as_deref(),
    };
    let draft = db::drafts::update(pool, upd.id, &patch)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::update: {e}")))?;
    audit(pool, "draft.update", Some(&upd.id.to_string()), &json!({})).await;
    encode_one(&draft)
}

async fn op_draft_delete(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    let removed = db::drafts::delete(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::delete: {e}")))?;
    audit(pool, "draft.delete", Some(&id.to_string()), &json!({})).await;
    Ok(json!({"removed": removed}))
}

// ---------- network ops -----------------------------------------------------

async fn op_account_test_login(
    pool: &SqlitePool,
    imap: &dyn ImapAuth,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let password = args
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'password'"))?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;

    let folders = imap
        .test_login(
            &account.imap_host,
            account.imap_port as u16,
            &account.email,
            password,
        )
        .await
        .map_err(|e| match e {
            crate::imap::ImapError::Auth(m) => RpcError::new("auth_failed", m),
            other => RpcError::internal(other.to_string()),
        })?;

    // Upsert what the server reported so folder.list reflects reality.
    for f in &folders {
        let role = role_for(&f.name);
        let _ = db::folders::upsert(
            pool,
            &db::folders::NewFolder {
                account_id,
                name: f.name.clone(),
                delimiter: f.delimiter.clone(),
                role,
                selectable: f.selectable,
            },
        )
        .await;
    }

    audit(
        pool,
        "account.test_login",
        Some(&account_id.to_string()),
        &json!({"folders": folders.len()}),
    )
    .await;

    Ok(json!({
        "ok": true,
        "folders": folders.iter().map(|f| &f.name).collect::<Vec<_>>(),
    }))
}

/// Map well-known IMAP folder names to the project's `FolderRole`.
fn role_for(name: &str) -> FolderRole {
    match name.to_ascii_uppercase().as_str() {
        "INBOX" => FolderRole::Inbox,
        "SENT" | "SENT ITEMS" | "[GMAIL]/SENT MAIL" => FolderRole::Sent,
        "DRAFTS" | "[GMAIL]/DRAFTS" => FolderRole::Drafts,
        "TRASH" | "DELETED ITEMS" | "[GMAIL]/TRASH" => FolderRole::Trash,
        "ARCHIVE" => FolderRole::Archive,
        "ALL MAIL" | "[GMAIL]/ALL MAIL" => FolderRole::All,
        "SPAM" | "JUNK" | "[GMAIL]/SPAM" => FolderRole::Spam,
        "STARRED" | "[GMAIL]/STARRED" => FolderRole::Starred,
        _ => FolderRole::Custom,
    }
}

async fn op_account_sync_folder(
    pool: &SqlitePool,
    hub: &Arc<Hub>,
    imap_sync: &dyn ImapSync,
    secrets: &dyn SecretStore,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    let secret = secrets
        .get(account_id)
        .await
        .map_err(|e| RpcError::internal(format!("secrets::get: {e}")))?
        .ok_or_else(|| RpcError::new("missing_secret", "no stored secret for account"))?;

    let report = sync::reconcile_folder(pool, hub, imap_sync, account_id, folder_name, &secret)
        .await
        .map_err(|e| match e {
            sync::SyncError::Imap(crate::imap::ImapError::Auth(m)) => {
                RpcError::new("auth_failed", m)
            }
            sync::SyncError::UnknownAccount => RpcError::bad_args("unknown account_id"),
            sync::SyncError::UnknownFolder(_) => RpcError::bad_args(e.to_string()),
            sync::SyncError::MissingCredentials => {
                RpcError::new("missing_secret", "no stored secret")
            }
            other => RpcError::internal(other.to_string()),
        })?;

    audit(
        pool,
        "account.sync_folder",
        Some(&account_id.to_string()),
        &json!({
            "folder_name": folder_name,
            "inserted": report.inserted,
            "wiped": report.wiped,
        }),
    )
    .await;

    encode_one(&report)
}

async fn op_account_start_sync(
    pool: &SqlitePool,
    secrets: &dyn SecretStore,
    manager: &sync::WorkerManager,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    ensure_account_folder(pool, account_id, folder_name).await?;

    let secret = secrets
        .get(account_id)
        .await
        .map_err(|e| RpcError::internal(format!("secrets::get: {e}")))?
        .ok_or_else(|| RpcError::new("missing_secret", "no stored secret for account"))?;

    let started = manager
        .start(account_id, folder_name.to_string(), secret)
        .await;
    audit(
        pool,
        "account.start_sync",
        Some(&account_id.to_string()),
        &json!({"folder_name": folder_name, "started": started}),
    )
    .await;
    Ok(json!({"ok": true, "started": started}))
}

async fn op_account_stop_sync(
    pool: &SqlitePool,
    manager: &sync::WorkerManager,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    let stopped = manager.stop(account_id, folder_name).await;
    audit(
        pool,
        "account.stop_sync",
        Some(&account_id.to_string()),
        &json!({"folder_name": folder_name, "stopped": stopped}),
    )
    .await;
    Ok(json!({"ok": true, "stopped": stopped}))
}

async fn ensure_account_folder(
    pool: &SqlitePool,
    account_id: uuid::Uuid,
    folder_name: &str,
) -> Result<(), RpcError> {
    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?;
    if account.is_none() {
        return Err(RpcError::bad_args("unknown account_id"));
    }

    let folder = db::folders::get_by_name(pool, account_id, folder_name)
        .await
        .map_err(|e| RpcError::internal(format!("folders::get_by_name: {e}")))?;
    if folder.is_none() {
        return Err(RpcError::bad_args(format!(
            "unknown folder '{folder_name}'"
        )));
    }

    Ok(())
}

// ---------- secret ops ------------------------------------------------------

async fn op_account_set_secret(
    pool: &SqlitePool,
    secrets: &dyn SecretStore,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let password = args
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'password'"))?;
    if password.is_empty() {
        return Err(RpcError::bad_args("'password' must be non-empty"));
    }

    // Verify the account exists before stashing the secret. Otherwise
    // a typo'd UUID would silently store an orphan entry.
    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?;
    if account.is_none() {
        return Err(RpcError::bad_args("unknown account_id"));
    }

    secrets
        .put(account_id, zeroize::Zeroizing::new(password.to_string()))
        .await
        .map_err(|e| RpcError::internal(format!("secrets::put: {e}")))?;

    audit(
        pool,
        "account.set_secret",
        Some(&account_id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"ok": true}))
}

async fn op_account_delete_secret(
    pool: &SqlitePool,
    secrets: &dyn SecretStore,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    secrets
        .delete(account_id)
        .await
        .map_err(|e| RpcError::internal(format!("secrets::delete: {e}")))?;
    audit(
        pool,
        "account.delete_secret",
        Some(&account_id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"ok": true}))
}

// ---------- helpers ---------------------------------------------------------

fn encode<T: serde::Serialize>(
    res: Result<T, sqlx::Error>,
    ctx: &'static str,
) -> Result<Value, RpcError> {
    let v = res.map_err(|e| RpcError::internal(format!("{ctx}: {e}")))?;
    serde_json::to_value(v).map_err(|e| RpcError::internal(format!("{ctx} encode: {e}")))
}

fn encode_one<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

fn parse_uuid(args: &Value, key: &str) -> Result<uuid::Uuid, RpcError> {
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args(format!("missing '{key}'")))?;
    uuid::Uuid::parse_str(s).map_err(|e| RpcError::bad_args(format!("bad '{key}': {e}")))
}

async fn audit(pool: &SqlitePool, action: &str, target: Option<&str>, details: &Value) {
    if let Err(e) = db::audit::record(
        pool,
        &db::audit::NewAuditEntry {
            actor: "user".into(),
            action: action.into(),
            target: target.map(|s| s.to_string()),
            details: details.clone(),
        },
    )
    .await
    {
        tracing::warn!(action, error = %e, "audit record failed");
    }
}
