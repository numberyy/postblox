//! `DaemonDispatcher` — the concrete `ipc::Dispatcher` impl.
//!
//! Maps wire op names to `db::*` calls and publishes events on the
//! [`Hub`] for write ops. No IMAP/SMTP yet — that wires in R3b.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Sqlite, SqlitePool, Transaction};
use thiserror::Error;

use crate::auth::MailCredential;
use crate::daemon::Op;
use crate::db;
use crate::imap::{self, ImapAuth, ImapError, ImapIdle, ImapSync};
use crate::ipc::{Dispatcher, Hub, RpcError, Topic};
use crate::models::{
    Account, AccountId, ApprovalState, AttachmentId, AuthKind, DraftId, FolderId, FolderRole,
    GateAction, MessageId, ThreadId,
};
use crate::oauth::google::{
    self, GoogleOAuth, GoogleOAuthConfig, GoogleOAuthError, GoogleOAuthHttpClient,
};
use crate::secrets::SecretStore;
use crate::smtp::{self, SmtpError, SmtpServer, SmtpSubmitRequest, SmtpSubmitter};
use crate::sync;

#[derive(Clone)]
pub struct DaemonServices {
    smtp: Arc<dyn SmtpSubmitter>,
    oauth: Arc<dyn GoogleOAuth>,
}

impl DaemonServices {
    pub fn new(smtp: Arc<dyn SmtpSubmitter>, oauth: Arc<dyn GoogleOAuth>) -> Self {
        Self { smtp, oauth }
    }

    pub fn with_smtp(smtp: Arc<dyn SmtpSubmitter>) -> Self {
        Self::new(smtp, Arc::new(GoogleOAuthHttpClient::new()))
    }
}

impl Default for DaemonServices {
    fn default() -> Self {
        Self::with_smtp(Arc::new(smtp::LettreSmtpSubmitter::new()))
    }
}

struct DaemonCredentialResolver {
    pool: SqlitePool,
    secrets: Arc<dyn SecretStore>,
    oauth: Arc<dyn GoogleOAuth>,
}

#[async_trait::async_trait]
impl sync::WorkerCredentialResolver for DaemonCredentialResolver {
    async fn resolve(&self, account_id: AccountId) -> Result<MailCredential, sync::SyncError> {
        let account = db::accounts::get(&self.pool, account_id)
            .await?
            .ok_or(sync::SyncError::UnknownAccount)?;
        credential_for_account(self.secrets.as_ref(), self.oauth.as_ref(), &account)
            .await
            .map_err(|err| {
                if err.code == "missing_secret" {
                    sync::SyncError::MissingCredentials
                } else {
                    sync::SyncError::Credential(format!("{}: {}", err.code, err.message))
                }
            })
    }
}

fn daemon_credential_resolver(
    pool: &SqlitePool,
    secrets: &Arc<dyn SecretStore>,
    oauth: &Arc<dyn GoogleOAuth>,
) -> Arc<dyn sync::WorkerCredentialResolver> {
    Arc::new(DaemonCredentialResolver {
        pool: pool.clone(),
        secrets: secrets.clone(),
        oauth: oauth.clone(),
    })
}

pub fn worker_manager_with_idle_config(
    pool: &SqlitePool,
    hub: &Arc<Hub>,
    imap_sync: Arc<dyn ImapSync>,
    idle: Option<Arc<dyn ImapIdle>>,
    secrets: &Arc<dyn SecretStore>,
    services: &DaemonServices,
    config: sync::WorkerConfig,
) -> Arc<sync::WorkerManager> {
    let credential_resolver = daemon_credential_resolver(pool, secrets, &services.oauth);
    Arc::new(
        sync::WorkerManager::with_idle_config_and_credential_resolver(
            pool.clone(),
            hub.clone(),
            imap_sync,
            idle,
            config,
            credential_resolver,
        ),
    )
}

#[derive(Clone)]
pub struct DaemonDispatcher {
    pool: SqlitePool,
    /// Sibling RO pool for the agent-facing `sql.query` / `sql.schema`
    /// surface. Cloned from `pool` in tests (in-memory SQLite has no
    /// separate RO connection); the daemon binary opens a real
    /// `mode=ro` pool via [`crate::db::connect_readonly`].
    read_pool: SqlitePool,
    hub: Arc<Hub>,
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    secrets: Arc<dyn SecretStore>,
    oauth: Arc<dyn GoogleOAuth>,
    smtp: Arc<dyn SmtpSubmitter>,
    worker_manager: Arc<sync::WorkerManager>,
}

/// Errors produced when constructing a [`DaemonDispatcher`] from
/// platform-default IMAP transports. Surfacing this lets callers
/// distinguish "rustls platform verifier failed" (recoverable: bad
/// system trust store) from a logic bug.
#[derive(Debug, Error)]
pub enum DispatcherInitError {
    #[error("imap transport init failed: {0}")]
    Imap(#[from] ImapError),
}

impl DaemonDispatcher {
    /// Production constructor: TLS-backed IMAP via rustls.
    pub fn new(
        pool: SqlitePool,
        read_pool: SqlitePool,
        hub: Arc<Hub>,
        secrets: Arc<dyn SecretStore>,
    ) -> Result<Self, DispatcherInitError> {
        let imap = imap::default_auth()?;
        let imap_sync = imap::default_sync()?;
        let imap_idle = imap::default_idle()?;
        let smtp = Arc::new(smtp::LettreSmtpSubmitter::new());
        let services = DaemonServices::with_smtp(smtp);
        let worker_manager = worker_manager_with_idle_config(
            &pool,
            &hub,
            imap_sync.clone(),
            Some(imap_idle),
            &secrets,
            &services,
            sync::WorkerConfig::default(),
        );
        Ok(Self::with_imap_smtp_oauth_and_manager(
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        ))
    }

    /// Test/customisation constructor: bring your own IMAP impls.
    pub fn with_imap(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        let smtp = Arc::new(smtp::LettreSmtpSubmitter::new());
        Self::with_imap_and_smtp(pool, hub, imap, imap_sync, secrets, smtp)
    }

    pub fn with_imap_and_smtp(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        smtp: Arc<dyn SmtpSubmitter>,
    ) -> Self {
        let services = DaemonServices::with_smtp(smtp);
        let worker_manager = worker_manager_with_idle_config(
            &pool,
            &hub,
            imap_sync.clone(),
            None,
            &secrets,
            &services,
            sync::WorkerConfig::default(),
        );
        let read_pool = pool.clone();
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
    }

    pub fn with_imap_and_sync_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        worker_config: sync::WorkerConfig,
    ) -> Self {
        let smtp = Arc::new(smtp::LettreSmtpSubmitter::new());
        Self::with_imap_sync_smtp_config(pool, hub, imap, imap_sync, secrets, smtp, worker_config)
    }

    pub fn with_imap_sync_smtp_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        smtp: Arc<dyn SmtpSubmitter>,
        worker_config: sync::WorkerConfig,
    ) -> Self {
        let services = DaemonServices::with_smtp(smtp);
        let worker_manager = worker_manager_with_idle_config(
            &pool,
            &hub,
            imap_sync.clone(),
            None,
            &secrets,
            &services,
            worker_config,
        );
        let read_pool = pool.clone();
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
    }

    pub fn with_imap_sync_smtp_oauth_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        services: DaemonServices,
        worker_config: sync::WorkerConfig,
    ) -> Self {
        let worker_manager = worker_manager_with_idle_config(
            &pool,
            &hub,
            imap_sync.clone(),
            None,
            &secrets,
            &services,
            worker_config,
        );
        let read_pool = pool.clone();
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
    }

    pub fn with_imap_and_manager(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        worker_manager: Arc<sync::WorkerManager>,
    ) -> Self {
        let smtp = Arc::new(smtp::LettreSmtpSubmitter::new());
        Self::with_imap_smtp_and_manager(pool, hub, imap, imap_sync, secrets, smtp, worker_manager)
    }

    pub fn with_imap_smtp_and_manager(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        smtp: Arc<dyn SmtpSubmitter>,
        worker_manager: Arc<sync::WorkerManager>,
    ) -> Self {
        let services = DaemonServices::with_smtp(smtp);
        let read_pool = pool.clone();
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_imap_smtp_oauth_and_manager(
        pool: SqlitePool,
        read_pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        services: DaemonServices,
        worker_manager: Arc<sync::WorkerManager>,
    ) -> Self {
        Self {
            pool,
            read_pool,
            hub,
            imap,
            imap_sync,
            secrets,
            oauth: services.oauth,
            smtp: services.smtp,
            worker_manager,
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl Dispatcher for DaemonDispatcher {
    async fn dispatch(&self, op: Op, args: Value) -> Result<Value, RpcError> {
        match op {
            // -- read ops --
            Op::AccountList => op_account_list(&self.pool).await,
            Op::FolderList => op_folder_list(&self.pool, args).await,
            Op::ThreadList => op_thread_list(&self.pool, args).await,
            Op::MessageListByFolder => op_messages_by_folder(&self.pool, args).await,
            Op::MessageListByThread => op_messages_by_thread(&self.pool, args).await,
            Op::MessageGet => op_message_get(&self.pool, args).await,
            Op::AttachmentList => op_attachment_list(&self.pool, args).await,
            Op::AttachmentPreview => op_attachment_preview(&self.pool, args).await,
            Op::Search => op_search(&self.pool, args).await,
            Op::SqlQuery => op_sql_query(&self.read_pool, args).await,
            Op::SqlSchema => op_sql_schema(&self.read_pool).await,
            Op::AuditListRecent => op_audit_list(&self.pool, args).await,

            // -- MCP gate/approval ops --
            Op::McpGateList => op_mcp_gate_list(&self.pool, args).await,
            Op::McpGateCreate => op_mcp_gate_create(&self.pool, args).await,
            Op::McpGateDelete => op_mcp_gate_delete(&self.pool, args).await,
            Op::McpApprovalCreate => op_mcp_approval_create(&self.pool, &self.hub, args).await,
            Op::McpApprovalList => op_mcp_approval_list(&self.pool, args).await,
            Op::McpApprovalGet => op_mcp_approval_get(&self.pool, args).await,
            Op::McpApprovalDecide => op_mcp_approval_decide(&self.pool, &self.hub, args).await,

            // -- write ops --
            Op::AccountCreate => op_account_create(&self.pool, args).await,
            Op::AccountDelete => op_account_delete(&self.pool, args).await,
            Op::FolderUpsert => op_folder_upsert(&self.pool, args).await,
            Op::MessageSetFlags => op_message_set_flags(&self.pool, &self.hub, args).await,
            Op::MessageArchive => op_message_archive(&self.pool, &self.hub, args).await,
            Op::MessageDelete => op_message_delete(&self.pool, &self.hub, args).await,
            Op::MessageMove => op_message_move(&self.pool, &self.hub, args).await,
            Op::DraftCreate => op_draft_create(&self.pool, args).await,
            Op::DraftUpdate => op_draft_update(&self.pool, args).await,
            Op::DraftDelete => op_draft_delete(&self.pool, args).await,
            Op::DraftList => op_draft_list(&self.pool, args).await,
            Op::DraftGet => op_draft_get(&self.pool, args).await,
            Op::AttachmentExport => op_attachment_export(&self.pool, args).await,
            Op::MessagePrepareReply => op_message_prepare_reply(&self.pool, args).await,
            Op::MessagePrepareForward => op_message_prepare_forward(&self.pool, args).await,
            Op::AttachmentFetchForForward => {
                op_attachment_fetch_for_forward(
                    &self.pool,
                    self.imap_sync.as_ref(),
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
            }
            Op::MessageSend => {
                op_message_send(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    self.smtp.as_ref(),
                    args,
                )
                .await
            }

            // -- network ops --
            Op::AccountTestLogin => {
                op_account_test_login(
                    &self.pool,
                    self.imap.as_ref(),
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
            }
            Op::AccountSyncFolder => {
                op_account_sync_folder(
                    &self.pool,
                    &self.hub,
                    self.imap_sync.as_ref(),
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
            }
            Op::AccountStartSync => {
                op_account_start_sync(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    self.worker_manager.as_ref(),
                    args,
                )
                .await
            }
            Op::AccountStopSync => {
                op_account_stop_sync(&self.pool, self.worker_manager.as_ref(), args).await
            }

            // -- secret ops --
            Op::AccountSetSecret => {
                op_account_set_secret(&self.pool, self.secrets.as_ref(), args).await
            }
            Op::AccountDeleteSecret => {
                op_account_delete_secret(&self.pool, self.secrets.as_ref(), args).await
            }

            // -- OAuth ops --
            Op::OauthGoogleAuthUrl => op_oauth_google_auth_url(&self.pool, args).await,
            Op::OauthGoogleComplete => {
                op_oauth_google_complete(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
            }
        }
    }
}

// ---------- read ops --------------------------------------------------------

async fn op_account_list(pool: &SqlitePool) -> Result<Value, RpcError> {
    encode(db::accounts::list(pool).await, "accounts::list")
}

async fn op_folder_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_id::<AccountId>(&args, "account_id")?;
    encode(
        db::folders::list_by_account(pool, id).await,
        "folders::list_by_account",
    )
}

async fn op_thread_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::threads::list_recent(pool, account_id, limit, offset).await,
        "threads::list_recent",
    )
}

async fn op_messages_by_folder(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let folder_id = parse_id::<FolderId>(&args, "folder_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::messages::list_by_folder(pool, folder_id, limit, offset).await,
        "messages::list_by_folder",
    )
}

async fn op_messages_by_thread(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let thread_id = parse_id::<ThreadId>(&args, "thread_id")?;
    encode(
        db::messages::list_by_thread(pool, thread_id).await,
        "messages::list_by_thread",
    )
}

async fn op_message_get(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_id::<MessageId>(&args, "id")?;
    encode(db::messages::get(pool, id).await, "messages::get")
}

async fn op_attachment_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let message_id = parse_id::<MessageId>(&args, "message_id")?;
    encode(
        db::attachments::list_for_message(pool, message_id).await,
        "attachments::list_for_message",
    )
}

async fn op_attachment_preview(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_id::<AttachmentId>(&args, "id")?;
    let attachment = db::attachments::get(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("attachments::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown attachment id"))?;
    let preview = crate::attachments::preview_attachment(attachment)
        .await
        .map_err(|e| RpcError::internal(format!("attachment preview: {e}")))?;
    encode_one(&preview)
}

async fn op_search(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let q = args
        .get("q")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'q'"))?;
    if q.trim().is_empty() {
        return Err(RpcError::bad_args("'q' must not be empty"));
    }
    let filters = db::search::SearchFilters {
        account_id: optional_id::<AccountId>(&args, "account_id")?,
        folder_id: optional_id::<FolderId>(&args, "folder_id")?,
        thread_id: optional_id::<ThreadId>(&args, "thread_id")?,
        date_from: optional_rfc3339(&args, "date_from")?,
        date_to: optional_rfc3339(&args, "date_to")?,
        from_addr: optional_nonempty_string(&args, "from_addr")?,
        to_addr: optional_nonempty_string(&args, "to_addr")?,
        has_attachments: optional_bool(&args, "has_attachments")?,
        unread: optional_bool(&args, "unread")?,
    };
    // Soft cap so a TUI search can't accidentally pull thousands of rows
    // over the IPC socket.
    let limit = args
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(50)
        .clamp(1, 200);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::search::search_filtered(pool, &db::search::quote_term(q), &filters, limit, offset)
            .await,
        "search",
    )
}

async fn op_sql_query(read_pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let sql = args
        .get("sql")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::bad_args("missing 'sql'"))?;
    let limit = args
        .get("limit")
        .map(|value| {
            value
                .as_u64()
                .filter(|limit| *limit > 0)
                .map(|limit| limit.min(crate::db::sql_query::MAX_ROWS as u64) as usize)
                .ok_or_else(|| RpcError::bad_args("'limit' must be a positive integer"))
        })
        .transpose()?
        .unwrap_or(crate::db::sql_query::DEFAULT_ROWS);
    match crate::db::sql_query::query(read_pool, sql, limit).await {
        Ok(rows) => serde_json::to_value(rows)
            .map_err(|e| RpcError::internal(format!("sql_query encode: {e}"))),
        Err(crate::db::sql_query::SqlError::Rejected { reason }) => Err(RpcError::bad_args(reason)),
        Err(crate::db::sql_query::SqlError::Sqlx(e)) => {
            Err(RpcError::internal(format!("sql_query: {e}")))
        }
    }
}

async fn op_sql_schema(read_pool: &SqlitePool) -> Result<Value, RpcError> {
    match crate::db::sql_query::schema(read_pool).await {
        Ok(rows) => serde_json::to_value(rows)
            .map_err(|e| RpcError::internal(format!("sql_schema encode: {e}"))),
        Err(crate::db::sql_query::SqlError::Sqlx(e)) => {
            Err(RpcError::internal(format!("sql_schema: {e}")))
        }
        Err(crate::db::sql_query::SqlError::Rejected { reason }) => {
            Err(RpcError::internal(format!("sql_schema rejected: {reason}")))
        }
    }
}

async fn op_audit_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::audit::list_recent(pool, limit, offset).await,
        "audit::list_recent",
    )
}

// ---------- MCP gate/approval ops ------------------------------------------

async fn op_mcp_gate_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    match args.get("tool").and_then(Value::as_str) {
        Some(tool) => encode(
            db::mcp::list_gates_for_tool(pool, tool).await,
            "mcp::list_gates_for_tool",
        ),
        None => encode(db::mcp::list_gates(pool).await, "mcp::list_gates"),
    }
}

async fn op_mcp_gate_create(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let tool = parse_str(&args, "tool")?;
    let action = parse_str(&args, "action")?
        .parse::<GateAction>()
        .map_err(RpcError::bad_args)?;
    let arg_pattern = optional_str(&args, "arg_pattern")?;
    if let Some(pattern) = arg_pattern {
        let parsed: Value = serde_json::from_str(pattern)
            .map_err(|e| RpcError::bad_args(format!("bad 'arg_pattern': {e}")))?;
        if !parsed.is_object() {
            return Err(RpcError::bad_args("'arg_pattern' must be a JSON object"));
        }
    }
    let note = optional_str(&args, "note")?;
    let gate = db::mcp::create_gate(pool, tool, arg_pattern, action, note)
        .await
        .map_err(|e| RpcError::internal(format!("mcp::create_gate: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "mcp.gate.create",
        Some(&gate.id.to_string()),
        &json!({"tool": tool, "action": action}),
    )
    .await;
    encode_one(&gate)
}

async fn op_mcp_gate_delete(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_uuid(&args, "id")?;
    let removed = db::mcp::delete_gate(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("mcp::delete_gate: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "mcp.gate.delete",
        Some(&id.to_string()),
        &json!({"removed": removed}),
    )
    .await;
    Ok(json!({"removed": removed}))
}

async fn op_mcp_approval_create(
    pool: &SqlitePool,
    hub: &Hub,
    args: Value,
) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let tool = parse_str(&args, "tool")?;
    let approval_args = args.get("args").cloned().unwrap_or_else(|| json!({}));
    let summary = parse_str(&args, "summary")?;
    let approval = db::mcp::create_approval(pool, tool, &approval_args, summary)
        .await
        .map_err(|e| RpcError::internal(format!("mcp::create_approval: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "mcp.approval.create",
        Some(&approval.id.to_string()),
        &json!({"tool": tool}),
    )
    .await;
    hub.publish(
        Topic::McpApprovalRequested,
        json!({
            "approval_id": approval.id,
            "tool": approval.tool,
            "summary": approval.summary,
            "state": approval.state,
            "args": approval.args,
        }),
    )
    .await;
    encode_one(&approval)
}

async fn op_mcp_approval_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let state = match args.get("state").and_then(Value::as_str) {
        Some(state) => Some(state.parse::<ApprovalState>().map_err(RpcError::bad_args)?),
        None => None,
    };
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    encode(
        db::mcp::list_approvals(pool, state, limit, offset).await,
        "mcp::list_approvals",
    )
}

async fn op_mcp_approval_get(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    encode(db::mcp::get_approval(pool, id).await, "mcp::get_approval")
}

async fn op_mcp_approval_decide(
    pool: &SqlitePool,
    hub: &Hub,
    args: Value,
) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_uuid(&args, "id")?;
    let state = parse_str(&args, "state")?
        .parse::<ApprovalState>()
        .map_err(RpcError::bad_args)?;
    if state == ApprovalState::Pending {
        return Err(RpcError::bad_args(
            "'state' must be allowed, denied, or expired",
        ));
    }
    let decided_by = args
        .get("decided_by")
        .and_then(Value::as_str)
        .unwrap_or(actor.as_str());
    let decided = db::mcp::decide(pool, id, state, decided_by)
        .await
        .map_err(|e| RpcError::internal(format!("mcp::decide: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "mcp.approval.decide",
        Some(&id.to_string()),
        &json!({"state": state, "decided": decided}),
    )
    .await;
    if decided {
        if let Some(approval) = db::mcp::get_approval(pool, id)
            .await
            .map_err(|e| RpcError::internal(format!("mcp::get_approval: {e}")))?
        {
            hub.publish(
                Topic::McpApprovalDecided,
                json!({
                    "approval_id": approval.id,
                    "tool": approval.tool,
                    "state": approval.state,
                    "decided_by": approval.decided_by,
                }),
            )
            .await;
        }
    }
    Ok(json!({"decided": decided}))
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
    let id = parse_id::<AccountId>(&args, "id")?;
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
    let actor = actor_from_args(&args);
    let id = parse_id::<MessageId>(&args, "id")?;
    let flags = args
        .get("flags")
        .cloned()
        .ok_or_else(|| RpcError::bad_args("missing 'flags'"))?;
    let outcome = db::messages::set_flags(pool, id, &flags)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_flags: {e}")))?;
    if outcome.changed {
        audit_actor(
            pool,
            &actor,
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
    }
    Ok(json!({"ok": true}))
}

async fn op_message_archive(pool: &SqlitePool, hub: &Hub, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_id::<MessageId>(&args, "id")?;
    let message = require_message(pool, id).await?;
    let archive = require_role_folder(pool, message.account_id, FolderRole::Archive).await?;
    if message.folder_id == archive.id {
        return Ok(json!({"ok": true, "folder_id": archive.id.to_string()}));
    }
    db::messages::set_folder(pool, id, archive.id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_folder: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "message.archive",
        Some(&id.to_string()),
        &json!({"folder_id": archive.id.to_string()}),
    )
    .await;
    hub.publish(
        Topic::MailUpdated,
        json!({
            "message_id": id.to_string(),
            "folder_id": archive.id.to_string(),
            "from_folder_id": message.folder_id.to_string(),
        }),
    )
    .await;
    Ok(json!({"ok": true, "folder_id": archive.id.to_string()}))
}

async fn op_message_delete(pool: &SqlitePool, hub: &Hub, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_id::<MessageId>(&args, "id")?;
    let message = require_message(pool, id).await?;
    let trash = require_role_folder(pool, message.account_id, FolderRole::Trash).await?;
    if message.folder_id == trash.id {
        return Ok(json!({"ok": true, "folder_id": trash.id.to_string()}));
    }
    db::messages::set_folder(pool, id, trash.id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_folder: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "message.delete",
        Some(&id.to_string()),
        &json!({"folder_id": trash.id.to_string()}),
    )
    .await;
    hub.publish(
        Topic::MailUpdated,
        json!({
            "message_id": id.to_string(),
            "folder_id": trash.id.to_string(),
            "from_folder_id": message.folder_id.to_string(),
        }),
    )
    .await;
    Ok(json!({"ok": true, "folder_id": trash.id.to_string()}))
}

async fn op_message_move(pool: &SqlitePool, hub: &Hub, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_id::<MessageId>(&args, "id")?;
    let folder_name = parse_str(&args, "folder_name")?;
    let message = require_message(pool, id).await?;
    let target = db::folders::get_by_name(pool, message.account_id, folder_name)
        .await
        .map_err(|e| RpcError::internal(format!("folders::get_by_name: {e}")))?
        .ok_or_else(|| RpcError::bad_args(format!("unknown folder '{folder_name}'")))?;
    if message.folder_id == target.id {
        return Ok(json!({"ok": true, "folder_id": target.id.to_string()}));
    }
    db::messages::set_folder(pool, id, target.id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_folder: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "message.move",
        Some(&id.to_string()),
        &json!({"folder_id": target.id.to_string(), "folder_name": folder_name}),
    )
    .await;
    hub.publish(
        Topic::MailUpdated,
        json!({
            "message_id": id.to_string(),
            "folder_id": target.id.to_string(),
            "from_folder_id": message.folder_id.to_string(),
        }),
    )
    .await;
    Ok(json!({"ok": true, "folder_id": target.id.to_string()}))
}

async fn require_message(
    pool: &SqlitePool,
    id: MessageId,
) -> Result<crate::models::Message, RpcError> {
    db::messages::get(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown message id"))
}

async fn require_role_folder(
    pool: &SqlitePool,
    account_id: AccountId,
    role: FolderRole,
) -> Result<crate::models::Folder, RpcError> {
    db::folders::list_by_account(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("folders::list_by_account: {e}")))?
        .into_iter()
        .find(|f| f.role == role)
        .ok_or_else(|| RpcError::bad_args(format!("no '{}' folder for account", role.as_str())))
}

async fn op_draft_create(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    // Pull attachments out before NewDraft deserialisation; the inner
    // type rejects unknown fields when deserialised standalone.
    let attachment_specs = take_attachment_specs(&args)?;
    let new: db::drafts::NewDraft =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let attachments = prepare_draft_attachments(attachment_specs).await?;
    let draft = db::drafts::create(pool, &new)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::create: {e}")))?;
    if let Some(attachments) = attachments {
        if let Err(e) = replace_draft_attachments(pool, draft.id, attachments).await {
            // best-effort rollback so callers can retry cleanly if attachment persistence fails.
            let _ = db::drafts::delete(pool, draft.id).await;
            return Err(e);
        }
    }
    audit_actor(
        pool,
        &actor,
        "draft.create",
        Some(&draft.id.to_string()),
        &json!({}),
    )
    .await;
    encode_one(&draft)
}

#[derive(Deserialize)]
struct DraftUpdate {
    id: DraftId,
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
    let actor = actor_from_args(&args);
    let attachment_specs = take_attachment_specs(&args)?;
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
    let attachments = prepare_draft_attachments(attachment_specs).await?;
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| RpcError::internal(format!("begin draft update transaction: {e}")))?;
    let draft = db::drafts::update_tx(&mut tx, upd.id, &patch)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::update: {e}")))?;
    // Only touch attachments when the caller explicitly supplied the
    // field; absence means "leave as-is" (matches the body/subject
    // partial-update contract).
    if let (Some(attachments), Some(_)) = (attachments, draft.as_ref()) {
        replace_draft_attachments_tx(&mut tx, upd.id, attachments).await?;
    }
    tx.commit()
        .await
        .map_err(|e| RpcError::internal(format!("commit draft update transaction: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "draft.update",
        Some(&upd.id.to_string()),
        &json!({}),
    )
    .await;
    encode_one(&draft)
}

#[derive(Debug, Deserialize)]
struct DraftAttachmentSpec {
    path: String,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    filename: Option<String>,
}

/// Pull the optional `attachments` array off a draft.create/update args
/// payload. Returns `None` if the field is absent or null, so callers
/// can distinguish "leave alone" from "replace with empty".
fn take_attachment_specs(args: &Value) -> Result<Option<Vec<DraftAttachmentSpec>>, RpcError> {
    match args.get("attachments") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let specs: Vec<DraftAttachmentSpec> = serde_json::from_value(value.clone())
                .map_err(|e| RpcError::bad_args(format!("bad 'attachments': {e}")))?;
            Ok(Some(specs))
        }
    }
}

#[derive(Debug)]
struct PendingDraftAttachment {
    path: PathBuf,
    original_path: String,
    metadata_size: u64,
    filename: String,
    content_type: String,
}

#[derive(Debug)]
struct PreparedDraftAttachment {
    filename: String,
    content_type: String,
    content: Vec<u8>,
}

async fn prepare_draft_attachments(
    specs: Option<Vec<DraftAttachmentSpec>>,
) -> Result<Option<Vec<PreparedDraftAttachment>>, RpcError> {
    let Some(specs) = specs else {
        return Ok(None);
    };

    let limit = db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES as u64;
    let mut aggregate = 0_u64;
    let mut pending = Vec::with_capacity(specs.len());
    for spec in specs {
        let path = PathBuf::from(&spec.path);
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| map_attachment_read_error(&spec.path, e))?;
        if !metadata.is_file() {
            return Err(RpcError::bad_args(format!(
                "not a regular file: {}",
                spec.path
            )));
        }
        let size = metadata.len();
        if size > limit {
            return Err(attachment_too_large_error(&spec.path, size));
        }
        aggregate = aggregate.saturating_add(size);
        if aggregate > limit {
            return Err(aggregate_attachments_too_large_error(aggregate));
        }
        let filename = spec.filename.unwrap_or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment.bin")
                .to_string()
        });
        let content_type = spec
            .content_type
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| crate::attachments::guess_content_type_for_path(&path));
        pending.push(PendingDraftAttachment {
            path,
            original_path: spec.path,
            metadata_size: size,
            filename,
            content_type,
        });
    }

    let mut actual_aggregate = 0_u64;
    let mut prepared = Vec::with_capacity(pending.len());
    for pending_attachment in pending {
        let content = read_attachment_bounded(
            &pending_attachment.path,
            &pending_attachment.original_path,
            pending_attachment.metadata_size,
        )
        .await?;
        actual_aggregate = actual_aggregate.saturating_add(content.len() as u64);
        if actual_aggregate > limit {
            return Err(aggregate_attachments_too_large_error(actual_aggregate));
        }
        prepared.push(PreparedDraftAttachment {
            filename: pending_attachment.filename,
            content_type: pending_attachment.content_type,
            content,
        });
    }

    Ok(Some(prepared))
}

async fn read_attachment_bounded(
    path: &Path,
    display_path: &str,
    metadata_size: u64,
) -> Result<Vec<u8>, RpcError> {
    use tokio::io::AsyncReadExt;

    let limit = db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES as u64;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| map_attachment_read_error(display_path, e))?;
    let mut reader = file.take(limit + 1);
    let mut bytes = Vec::with_capacity(metadata_size.min(limit) as usize);
    reader
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| map_attachment_read_error(display_path, e))?;
    if bytes.len() as u64 > limit {
        return Err(attachment_too_large_error(display_path, bytes.len() as u64));
    }
    Ok(bytes)
}

fn attachment_too_large_error(path: &str, size: u64) -> RpcError {
    RpcError::new(
        "attachment_too_large",
        format!(
            "attachment '{path}' is {size} bytes, exceeds {} byte limit",
            db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES
        ),
    )
}

fn aggregate_attachments_too_large_error(size: u64) -> RpcError {
    RpcError::new(
        "attachment_too_large",
        format!(
            "aggregate draft attachments {size} bytes exceed {} byte limit",
            db::draft_attachments::MAX_DRAFT_ATTACHMENT_BYTES
        ),
    )
}

async fn replace_draft_attachments(
    pool: &SqlitePool,
    draft_id: DraftId,
    attachments: Vec<PreparedDraftAttachment>,
) -> Result<(), RpcError> {
    db::draft_attachments::delete_all_for_draft(pool, draft_id)
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::delete_all: {e}")))?;
    for attachment in attachments {
        db::draft_attachments::create(
            pool,
            &db::draft_attachments::NewDraftAttachment {
                draft_id,
                filename: attachment.filename,
                content_type: attachment.content_type,
                content: attachment.content,
            },
        )
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::create: {e}")))?;
    }
    Ok(())
}

async fn replace_draft_attachments_tx(
    tx: &mut Transaction<'_, Sqlite>,
    draft_id: DraftId,
    attachments: Vec<PreparedDraftAttachment>,
) -> Result<(), RpcError> {
    db::draft_attachments::delete_all_for_draft_tx(tx, draft_id)
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::delete_all: {e}")))?;
    for attachment in attachments {
        db::draft_attachments::create_tx(
            tx,
            &db::draft_attachments::NewDraftAttachment {
                draft_id,
                filename: attachment.filename,
                content_type: attachment.content_type,
                content: attachment.content,
            },
        )
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::create: {e}")))?;
    }
    Ok(())
}

fn map_attachment_read_error(path: &str, err: std::io::Error) -> RpcError {
    match err.kind() {
        std::io::ErrorKind::NotFound => RpcError::bad_args(format!("file not found: {path}")),
        std::io::ErrorKind::PermissionDenied => {
            RpcError::bad_args(format!("permission denied: {path}"))
        }
        _ => RpcError::bad_args(format!("cannot read {path}: {err}")),
    }
}

async fn op_draft_delete(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_id::<DraftId>(&args, "id")?;
    let removed = db::drafts::delete(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::delete: {e}")))?;
    audit_actor(
        pool,
        &actor,
        "draft.delete",
        Some(&id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"removed": removed}))
}

async fn op_draft_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    encode(
        db::drafts::list_by_account(pool, account_id).await,
        "drafts::list_by_account",
    )
}

async fn op_draft_get(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    use base64::Engine;

    let id = parse_id::<DraftId>(&args, "id")?;
    let draft = db::drafts::get(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::get: {e}")))?;
    let Some(draft) = draft else {
        return Ok(Value::Null);
    };
    let attachment_rows = db::draft_attachments::list_for_draft(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::list_for_draft: {e}")))?;
    let mut attachments_json = Vec::with_capacity(attachment_rows.len());
    for row in attachment_rows {
        let content = db::draft_attachments::load_content(pool, row.id)
            .await
            .map_err(|e| RpcError::internal(format!("draft_attachments::load_content: {e}")))?;
        let bytes = require_draft_attachment_content(row.id, content)?;
        attachments_json.push(json!({
            "id": row.id.to_string(),
            "draft_id": row.draft_id.to_string(),
            "filename": row.filename,
            "content_type": row.content_type,
            "size_bytes": row.size_bytes,
            "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        }));
    }
    Ok(json!({
        "draft": draft,
        "attachments": attachments_json,
    }))
}

fn require_draft_attachment_content(
    attachment_id: uuid::Uuid,
    content: Option<Vec<u8>>,
) -> Result<Vec<u8>, RpcError> {
    content.ok_or_else(|| {
        RpcError::internal(format!(
            "draft attachment content missing for {attachment_id}"
        ))
    })
}

fn message_view(message: &crate::models::Message) -> crate::mail::reply::MessageView<'_> {
    crate::mail::reply::MessageView {
        id: message.id.into_inner(),
        from_addr: &message.from_addr,
        reply_to: message.reply_to.as_deref(),
        subject: message.subject.as_deref(),
        message_id_header: message.message_id_header.as_deref(),
        references_header: message.references_header.as_deref(),
        to_addrs: &message.to_addrs,
        cc_addrs: &message.cc_addrs,
        text_body: message.text_body.as_deref(),
        html_body: message.html_body.as_deref(),
        internal_date: message.internal_date,
    }
}

/// Build a `ReplyDraft` for the given message + responding account.
///
/// Pure-data op: the daemon doesn't persist anything yet — the TUI
/// uses the response to pre-fill the composer, and persistence happens
/// when the composer first auto-saves through `draft.create`.
async fn op_message_prepare_reply(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let message_id = parse_id::<MessageId>(&args, "message_id")?;
    let reply_all = args
        .get("reply_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let message = require_message(pool, message_id).await?;
    let account = db::accounts::get(pool, message.account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account for message"))?;
    let draft = crate::mail::reply::reply_draft(message_view(&message), &account.email, reply_all);
    Ok(json!({
        "message_id": message.id.to_string(),
        "account_id": account.id.to_string(),
        "to": draft.to,
        "cc": draft.cc,
        "subject": draft.subject,
        "in_reply_to": draft.in_reply_to,
        "references": draft.references,
        "quoted_body": draft.quoted_body,
    }))
}

/// Build a `ForwardDraft` plus a manifest of the original
/// attachments. The TUI then asks `attachment.fetch_for_forward` per
/// entry to materialise bytes before the user sends.
async fn op_message_prepare_forward(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let message_id = parse_id::<MessageId>(&args, "message_id")?;
    let message = require_message(pool, message_id).await?;
    let account = db::accounts::get(pool, message.account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account for message"))?;
    let attachments = db::attachments::list_for_message(pool, message.id)
        .await
        .map_err(|e| RpcError::internal(format!("attachments::list_for_message: {e}")))?;
    let attachment_tuples: Vec<(uuid::Uuid, String, String, i64)> = attachments
        .into_iter()
        .map(|a| (a.id.into_inner(), a.filename, a.content_type, a.size_bytes))
        .collect();
    let draft = crate::mail::reply::forward_draft(message_view(&message), &attachment_tuples);
    let attachments_json: Vec<Value> = draft
        .forwarded_attachments
        .iter()
        .map(|a| {
            json!({
                "message_id": a.message_id.to_string(),
                "attachment_id": a.attachment_id.to_string(),
                "filename": a.filename,
                "content_type": a.content_type,
                "size_bytes": a.size_bytes,
            })
        })
        .collect();
    Ok(json!({
        "message_id": message.id.to_string(),
        "account_id": account.id.to_string(),
        "subject": draft.subject,
        "forwarded_body": draft.forwarded_body,
        "forwarded_attachments": attachments_json,
    }))
}

async fn op_attachment_export(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_id::<AttachmentId>(&args, "id")?;
    let destination_path = parse_str(&args, "destination_path")?;
    let attachment = db::attachments::get(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("attachments::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown attachment id"))?;
    let exported = crate::attachments::export_attachment(&attachment, Path::new(destination_path))
        .await
        .map_err(map_attachment_export_error)?;
    audit_actor(
        pool,
        &actor,
        "attachment.export",
        Some(&id.to_string()),
        &json!({"destination_path": exported.destination_path}),
    )
    .await;
    encode_one(&exported)
}

/// Fetch the bytes of an attachment so the forward composer can carry
/// it forward. Returns the cached copy when available; falls back to
/// re-fetching the parent message via IMAP and re-parsing for the
/// matching part. If neither path produces bytes the response is
/// `unavailable_offline`.
async fn op_attachment_fetch_for_forward(
    pool: &SqlitePool,
    imap_sync: &dyn ImapSync,
    secrets: &dyn SecretStore,
    oauth: &dyn GoogleOAuth,
    args: Value,
) -> Result<Value, RpcError> {
    use base64::Engine;

    let attachment_id = parse_id::<AttachmentId>(&args, "attachment_id")?;
    let attachment = db::attachments::get(pool, attachment_id)
        .await
        .map_err(|e| RpcError::internal(format!("attachments::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown attachment id"))?;

    if let Ok(bytes) = tokio::fs::read(&attachment.storage_path).await {
        return Ok(json!({
            "attachment_id": attachment.id.to_string(),
            "filename": attachment.filename,
            "content_type": attachment.content_type,
            "size_bytes": bytes.len() as i64,
            "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
            "source": "cache",
        }));
    }

    let message = db::messages::get(pool, attachment.message_id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::get: {e}")))?
        .ok_or_else(|| {
            RpcError::new(
                "unavailable_offline",
                "attachment unavailable offline (parent message missing)",
            )
        })?;
    // folder and account are independent given `message`; fetch concurrently.
    let (folder, account) = tokio::try_join!(
        db::folders::get(pool, message.folder_id),
        db::accounts::get(pool, message.account_id),
    )
    .map_err(|e| RpcError::internal(format!("folders/accounts::get: {e}")))?;
    let folder = folder.ok_or_else(|| {
        RpcError::new(
            "unavailable_offline",
            "attachment unavailable offline (folder missing)",
        )
    })?;
    let account = account.ok_or_else(|| {
        RpcError::new(
            "unavailable_offline",
            "attachment unavailable offline (account missing)",
        )
    })?;

    let credential = match credential_for_account(secrets, oauth, &account).await {
        Ok(credential) => credential,
        Err(_) => {
            return Err(RpcError::new(
                "unavailable_offline",
                "attachment unavailable offline",
            ))
        }
    };
    let target_uid = u32::try_from(message.uid)
        .map_err(|_| RpcError::new("unavailable_offline", "attachment unavailable offline"))?;
    let server = match imap_sync
        .sync_folder(
            &account.imap_host,
            account.imap_port as u16,
            &account.email,
            &credential,
            &folder.name,
            target_uid,
        )
        .await
    {
        Ok(server) => server,
        Err(_) => {
            return Err(RpcError::new(
                "unavailable_offline",
                "attachment unavailable offline",
            ))
        }
    };
    let fetched = server
        .messages
        .into_iter()
        .find(|m| m.uid == target_uid)
        .ok_or_else(|| RpcError::new("unavailable_offline", "attachment unavailable offline"))?;
    let parsed = crate::mail::parser::parse(&fetched.raw)
        .map_err(|_| RpcError::new("unavailable_offline", "attachment unavailable offline"))?;
    let bytes = pick_attachment_bytes(parsed.attachments, &attachment)
        .ok_or_else(|| RpcError::new("unavailable_offline", "attachment unavailable offline"))?;

    if let Err(e) = persist_refetched_bytes(&attachment.storage_path, &bytes).await {
        tracing::warn!(error = %e, "could not cache refetched attachment bytes");
    }

    Ok(json!({
        "attachment_id": attachment.id.to_string(),
        "filename": attachment.filename,
        "content_type": attachment.content_type,
        "size_bytes": bytes.len() as i64,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "source": "imap",
    }))
}

fn pick_attachment_bytes(
    mut parsed: Vec<crate::mail::parser::ParsedAttachment>,
    attachment: &crate::models::Attachment,
) -> Option<Vec<u8>> {
    let cid = attachment.content_id.as_deref().filter(|s| !s.is_empty());
    if let Some(cid) = cid {
        if let Some(idx) = parsed
            .iter()
            .position(|p| p.content_id.as_deref() == Some(cid))
        {
            return Some(parsed.swap_remove(idx).data);
        }
    }
    if let Some(idx) = parsed.iter().position(|p| {
        p.filename == attachment.filename && p.content_type == attachment.content_type
    }) {
        return Some(parsed.swap_remove(idx).data);
    }
    let idx = parsed
        .iter()
        .position(|p| p.filename == attachment.filename)?;
    Some(parsed.swap_remove(idx).data)
}

async fn persist_refetched_bytes(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    let target = Path::new(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    tokio::fs::write(target, bytes).await
}

fn map_attachment_export_error(err: std::io::Error) -> RpcError {
    match err.kind() {
        std::io::ErrorKind::AlreadyExists => RpcError::bad_args(err.to_string()),
        std::io::ErrorKind::NotFound => RpcError::bad_args(err.to_string()),
        std::io::ErrorKind::InvalidInput => RpcError::bad_args(err.to_string()),
        _ => RpcError::internal(format!("attachment export: {err}")),
    }
}

async fn op_message_send(
    pool: &SqlitePool,
    secrets: &dyn SecretStore,
    oauth: &dyn GoogleOAuth,
    smtp: &dyn SmtpSubmitter,
    args: Value,
) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let draft_id = parse_id::<DraftId>(&args, "draft_id")?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    let draft = db::drafts::get(pool, draft_id)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown draft_id"))?;
    if draft.account_id != account_id {
        return Err(RpcError::bad_args("draft does not belong to account"));
    }

    let to = parse_addr_array(&draft.to_addrs, "to_addrs")?;
    let cc = parse_addr_array(&draft.cc_addrs, "cc_addrs")?;
    let bcc = parse_addr_array(&draft.bcc_addrs, "bcc_addrs")?;
    let recipients = all_recipients(&to, &cc, &bcc)?;
    let smtp_port =
        u16::try_from(account.smtp_port).map_err(|_| RpcError::bad_args("bad smtp_port"))?;

    let credential = credential_for_account(secrets, oauth, &account).await?;

    let message_id = format!("<{}@postblox.local>", uuid::Uuid::new_v4());
    let subject = draft.subject.as_deref().unwrap_or("");
    let attachments = load_draft_mime_attachments(pool, draft_id).await?;
    let attachment_count = attachments.len();
    let reply = crate::mail::builder::ReplyHeaders {
        in_reply_to: draft.in_reply_to.as_deref(),
        references: draft.references_header.as_deref(),
    };
    let mime = crate::mail::builder::build_mime_full(crate::mail::builder::MimeBuildOptions {
        from: &account.email,
        to: &to,
        cc: &cc,
        subject,
        text_body: draft.text_body.as_deref(),
        html_body: draft.html_body.as_deref(),
        message_id: &message_id,
        attachments: &attachments,
        reply,
    });

    smtp.submit(SmtpSubmitRequest {
        server: SmtpServer {
            host: account.smtp_host.clone(),
            port: smtp_port,
            use_tls: account.smtp_use_tls,
            starttls: account.smtp_starttls,
        },
        username: account.email.clone(),
        credential,
        from: account.email.clone(),
        recipients,
        mime,
    })
    .await
    .map_err(map_smtp_error)?;

    // Drop the local draft now that the wire has accepted it.
    // Cascade clears `draft_attachments` rows automatically. Errors
    // here are logged but don't fail the send — the user shouldn't
    // see "send failed" once SMTP returned 250.
    if let Err(e) = db::drafts::delete(pool, draft_id).await {
        tracing::warn!(
            error = %e,
            draft_id = %draft_id,
            "drafts::delete after send failed"
        );
    }

    audit_actor(
        pool,
        &actor,
        "message.send",
        Some(&draft_id.to_string()),
        &json!({
            "account_id": account_id,
            "message_id": message_id,
            "attachment_count": attachment_count,
        }),
    )
    .await;
    Ok(json!({"ok": true, "message_id": message_id}))
}

async fn load_draft_mime_attachments(
    pool: &SqlitePool,
    draft_id: DraftId,
) -> Result<Vec<crate::mail::builder::MimeAttachment>, RpcError> {
    let rows = db::draft_attachments::list_for_draft(pool, draft_id)
        .await
        .map_err(|e| RpcError::internal(format!("draft_attachments::list_for_draft: {e}")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let bytes = db::draft_attachments::load_content(pool, row.id)
            .await
            .map_err(|e| RpcError::internal(format!("draft_attachments::load_content: {e}")))?
            .ok_or_else(|| {
                RpcError::internal(format!("draft attachment {} disappeared mid-send", row.id))
            })?;
        out.push(crate::mail::builder::MimeAttachment {
            filename: row.filename,
            content_type: row.content_type,
            data: bytes,
            content_id: None,
        });
    }
    Ok(out)
}

fn parse_addr_array(value: &Value, field: &str) -> Result<Vec<String>, RpcError> {
    let values = value
        .as_array()
        .ok_or_else(|| RpcError::bad_args(format!("{field} must be an array")))?;
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let addr = value
            .as_str()
            .ok_or_else(|| RpcError::bad_args(format!("{field} must contain strings")))?;
        let addr = addr.trim();
        if addr.is_empty() {
            return Err(RpcError::bad_args(format!(
                "{field} must not contain empty addresses"
            )));
        }
        out.push(addr.to_string());
    }
    Ok(out)
}

fn all_recipients(to: &[String], cc: &[String], bcc: &[String]) -> Result<Vec<String>, RpcError> {
    let recipients = to
        .iter()
        .chain(cc.iter())
        .chain(bcc.iter())
        .cloned()
        .collect::<Vec<_>>();
    if recipients.is_empty() {
        Err(RpcError::bad_args("at least one recipient is required"))
    } else {
        Ok(recipients)
    }
}

fn map_smtp_error(err: SmtpError) -> RpcError {
    match err {
        SmtpError::Auth(message) => RpcError::new("auth_failed", message),
        SmtpError::InvalidRequest(message) => RpcError::bad_args(message),
        SmtpError::InvalidConfig(message)
        | SmtpError::Transient(message)
        | SmtpError::Internal(message) => RpcError::internal(message),
    }
}

// ---------- network ops -----------------------------------------------------

async fn op_account_test_login(
    pool: &SqlitePool,
    imap: &dyn ImapAuth,
    secrets: &dyn SecretStore,
    oauth: &dyn GoogleOAuth,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    let credential = match account.auth_kind {
        AuthKind::Password => {
            let password = args
                .get("password")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError::bad_args("missing 'password'"))?;
            MailCredential::password(password)
        }
        AuthKind::OAuth2Google => credential_for_account(secrets, oauth, &account).await?,
    };

    let folders = imap
        .test_login(
            &account.imap_host,
            account.imap_port as u16,
            &account.email,
            &credential,
        )
        .await
        .map_err(|e| match e {
            crate::imap::ImapError::Auth(m) => RpcError::new("auth_failed", m),
            other => RpcError::internal(other.to_string()),
        })?;

    // Upsert what the server reported so folder.list reflects reality.
    for f in &folders {
        let role = role_for(&f.name);
        if let Err(error) = db::folders::upsert(
            pool,
            &db::folders::NewFolder {
                account_id,
                name: f.name.clone(),
                delimiter: f.delimiter.clone(),
                role,
                selectable: f.selectable,
            },
        )
        .await
        {
            tracing::warn!(
                account_id = %account_id,
                folder_name = %f.name,
                %error,
                "failed to upsert folder during test login"
            );
        }
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
    oauth: &dyn GoogleOAuth,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    let credential = credential_for_account(secrets, oauth, &account).await?;

    sync::publish_sync_state(
        hub,
        sync::SyncStateEvent::new(account_id, sync::SyncState::Syncing, None),
    )
    .await;
    let report =
        sync::reconcile_folder(pool, hub, imap_sync, account_id, folder_name, &credential).await;
    let report = match report {
        Ok(report) => {
            sync::publish_sync_state(
                hub,
                sync::SyncStateEvent::new(account_id, sync::SyncState::Idle, None),
            )
            .await;
            report
        }
        Err(e) => {
            sync::publish_sync_state(
                hub,
                sync::SyncStateEvent::new(account_id, sync::SyncState::Error, Some(e.to_string())),
            )
            .await;
            return Err(match e {
                sync::SyncError::Imap(crate::imap::ImapError::Auth(m)) => {
                    RpcError::new("auth_failed", m)
                }
                sync::SyncError::UnknownAccount => RpcError::bad_args("unknown account_id"),
                sync::SyncError::UnknownFolder(_) => RpcError::bad_args(e.to_string()),
                sync::SyncError::MissingCredentials => {
                    RpcError::new("missing_secret", "no stored secret")
                }
                other => RpcError::internal(other.to_string()),
            });
        }
    };

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
    oauth: &dyn GoogleOAuth,
    manager: &sync::WorkerManager,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    ensure_account_folder(pool, account_id, folder_name).await?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    let credential = credential_for_account(secrets, oauth, &account).await?;

    let started = manager
        .start(account_id, folder_name.to_string(), credential)
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
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
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
    account_id: AccountId,
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
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
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
    let account = account.ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    if account.auth_kind != AuthKind::Password {
        return Err(RpcError::bad_args(
            "account.set_secret only supports password accounts",
        ));
    }

    secrets
        .put(account_id, zeroize::Zeroizing::new(password.to_string()))
        .await
        .map_err(|e| RpcError::internal(format!("secrets::put: {e}")))?;
    db::accounts::set_secret_ref(pool, account_id, Some(&secrets.secret_ref(account_id)))
        .await
        .map_err(|e| RpcError::internal(format!("accounts::set_secret_ref: {e}")))?;

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
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    secrets
        .delete(account_id)
        .await
        .map_err(|e| RpcError::internal(format!("secrets::delete: {e}")))?;
    db::accounts::set_secret_ref(pool, account_id, None)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::set_secret_ref: {e}")))?;
    audit(
        pool,
        "account.delete_secret",
        Some(&account_id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"ok": true}))
}

// ---------- OAuth ops -------------------------------------------------------

async fn op_oauth_google_auth_url(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let client_id = parse_str(&args, "client_id")?;
    let redirect_uri = parse_str(&args, "redirect_uri")?;
    let state = parse_str(&args, "state")?;
    ensure_oauth_google_account(pool, account_id).await?;
    ensure_requested_scopes_are_gmail(&args)?;

    let config = GoogleOAuthConfig::gmail(client_id, "", redirect_uri);
    let url = google::authorization_url(&config, state).map_err(map_oauth_error)?;
    audit(
        pool,
        "oauth.google.auth_url",
        Some(&account_id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"authorization_url": url}))
}

async fn op_oauth_google_complete(
    pool: &SqlitePool,
    secrets: &dyn SecretStore,
    oauth: &dyn GoogleOAuth,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_id::<AccountId>(&args, "account_id")?;
    let client_id = parse_str(&args, "client_id")?;
    let client_secret = parse_str(&args, "client_secret")?;
    let redirect_uri = parse_str(&args, "redirect_uri")?;
    let code = parse_str(&args, "code")?;
    let state = parse_str(&args, "state")?;
    let expected_state = parse_str(&args, "expected_state")?;
    if state != expected_state {
        return Err(RpcError::bad_args(
            "'state' does not match 'expected_state'",
        ));
    }
    ensure_oauth_google_account(pool, account_id).await?;
    ensure_requested_scopes_are_gmail(&args)?;

    let config = GoogleOAuthConfig::gmail(client_id, client_secret, redirect_uri);
    let token = oauth
        .exchange_code(&config, code)
        .await
        .map_err(map_oauth_error)?;
    let expires_at = token.expires_at;
    let stored = google::StoredGoogleOAuth::new(config, token);
    google::store_stored_oauth(secrets, account_id, &stored)
        .await
        .map_err(map_oauth_error)?;
    db::accounts::set_secret_ref(pool, account_id, Some(&secrets.secret_ref(account_id)))
        .await
        .map_err(|e| RpcError::internal(format!("accounts::set_secret_ref: {e}")))?;

    audit(
        pool,
        "oauth.google.complete",
        Some(&account_id.to_string()),
        &json!({}),
    )
    .await;
    Ok(json!({"ok": true, "expires_at": expires_at}))
}

async fn ensure_oauth_google_account(
    pool: &SqlitePool,
    account_id: AccountId,
) -> Result<(), RpcError> {
    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    if account.auth_kind != AuthKind::OAuth2Google {
        return Err(RpcError::bad_args(
            "account auth_kind must be oauth2_google",
        ));
    }
    Ok(())
}

fn ensure_requested_scopes_are_gmail(args: &Value) -> Result<(), RpcError> {
    let Some(scopes) = parse_optional_scopes(args)? else {
        return Ok(());
    };
    if scopes.len() == 1 && scopes[0] == google::GMAIL_SCOPE {
        Ok(())
    } else {
        Err(RpcError::bad_args(
            "only the Gmail OAuth scope is supported",
        ))
    }
}

fn parse_optional_scopes(args: &Value) -> Result<Option<Vec<String>>, RpcError> {
    let Some(value) = args.get("scopes") else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| RpcError::bad_args("'scopes' must be an array"))?;
    let mut scopes = Vec::with_capacity(values.len());
    for value in values {
        let scope = value
            .as_str()
            .ok_or_else(|| RpcError::bad_args("'scopes' must contain strings"))?
            .trim();
        if scope.is_empty() {
            return Err(RpcError::bad_args(
                "'scopes' must not contain empty strings",
            ));
        }
        scopes.push(scope.to_string());
    }
    Ok(Some(scopes))
}

async fn credential_for_account(
    secrets: &dyn SecretStore,
    oauth: &dyn GoogleOAuth,
    account: &Account,
) -> Result<MailCredential, RpcError> {
    match account.auth_kind {
        AuthKind::Password => {
            let secret = secrets
                .get(account.id)
                .await
                .map_err(|e| RpcError::internal(format!("secrets::get: {e}")))?
                .ok_or_else(|| RpcError::new("missing_secret", "no stored secret for account"))?;
            Ok(MailCredential::password_secret(secret))
        }
        AuthKind::OAuth2Google => {
            let mut stored = google::load_stored_oauth(secrets, account.id)
                .await
                .map_err(map_oauth_error)?
                .ok_or_else(|| {
                    RpcError::new("missing_secret", "no stored OAuth token for account")
                })?;
            if stored.token.needs_refresh(chrono::Utc::now()) {
                let refreshed = oauth
                    .refresh_token(&stored.config(), &stored.token)
                    .await
                    .map_err(map_oauth_error)?;
                stored.token = refreshed;
                google::store_stored_oauth(secrets, account.id, &stored)
                    .await
                    .map_err(map_oauth_error)?;
            }
            Ok(MailCredential::oauth2_bearer(stored.token.access_token))
        }
    }
}

fn map_oauth_error(err: GoogleOAuthError) -> RpcError {
    match err {
        GoogleOAuthError::InvalidInput(message) => RpcError::bad_args(message),
        GoogleOAuthError::MissingRefreshToken => RpcError::new(
            "oauth_failed",
            "OAuth response did not include refresh token",
        ),
        GoogleOAuthError::HttpStatus(status) => RpcError::new(
            "oauth_failed",
            format!("OAuth token endpoint returned status {status}"),
        ),
        GoogleOAuthError::Http(err) => RpcError::internal(format!("oauth http: {err}")),
        GoogleOAuthError::Secret(err) => RpcError::internal(format!("oauth secrets: {err}")),
        GoogleOAuthError::Decode(err) => RpcError::internal(format!("oauth decode: {err}")),
    }
}

// ---------- helpers ---------------------------------------------------------

fn encode<T: serde::Serialize>(
    res: Result<T, db::DbError>,
    ctx: &'static str,
) -> Result<Value, RpcError> {
    let v = res.map_err(|e| RpcError::internal(format!("{ctx}: {e}")))?;
    serde_json::to_value(v).map_err(|e| RpcError::internal(format!("{ctx} encode: {e}")))
}

fn encode_one<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

fn parse_uuid(args: &Value, key: &str) -> Result<uuid::Uuid, RpcError> {
    parse_id::<uuid::Uuid>(args, key)
}

fn parse_id<T>(args: &Value, key: &str) -> Result<T, RpcError>
where
    T: std::str::FromStr<Err = uuid::Error>,
{
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args(format!("missing '{key}'")))?;
    s.parse::<T>()
        .map_err(|e| RpcError::bad_args(format!("bad '{key}': {e}")))
}

fn parse_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::bad_args(format!("missing '{key}'")))
}

fn optional_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, RpcError> {
    match args.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(RpcError::bad_args(format!("'{key}' must be a string"))),
    }
}

fn optional_id<T>(args: &Value, key: &str) -> Result<Option<T>, RpcError>
where
    T: std::str::FromStr<Err = uuid::Error>,
{
    match args.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(_) => Ok(Some(parse_id(args, key)?)),
    }
}

fn optional_bool(args: &Value, key: &str) -> Result<Option<bool>, RpcError> {
    match args.get(key) {
        Some(Value::Null) | None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(RpcError::bad_args(format!("'{key}' must be a boolean"))),
    }
}

fn optional_rfc3339(
    args: &Value,
    key: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, RpcError> {
    let Some(s) = optional_str(args, key)? else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(s)
        .map_err(|e| RpcError::bad_args(format!("'{key}' must be rfc3339: {e}")))?;
    Ok(Some(parsed.with_timezone(&chrono::Utc)))
}

fn optional_nonempty_string(args: &Value, key: &str) -> Result<Option<String>, RpcError> {
    let Some(s) = optional_str(args, key)? else {
        return Ok(None);
    };
    if s.is_empty() {
        return Err(RpcError::bad_args(format!("'{key}' must not be empty")));
    }
    Ok(Some(s.to_string()))
}

fn actor_from_args(args: &Value) -> String {
    args.get("_actor")
        .and_then(Value::as_str)
        .filter(|actor| actor_is_allowed(actor))
        .unwrap_or("user")
        .to_string()
}

fn actor_is_allowed(actor: &str) -> bool {
    actor == "user"
        || (actor
            .strip_prefix("mcp:")
            .is_some_and(|name| !name.is_empty() && name.len() <= 128))
}

async fn audit(pool: &SqlitePool, action: &str, target: Option<&str>, details: &Value) {
    audit_actor(pool, "user", action, target, details).await;
}

async fn audit_actor(
    pool: &SqlitePool,
    actor: &str,
    action: &str,
    target: Option<&str>,
    details: &Value,
) {
    if let Err(e) = db::audit::record(
        pool,
        &db::audit::NewAuditEntry {
            actor: actor.into(),
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

#[cfg(test)]
mod tests {
    //! Inline coverage for the lowest-level dispatcher units — argument
    //! parsing helpers and a handful of DB-only op handlers. End-to-end
    //! coverage over the IPC seam lives in `tests/ipc_integration.rs`;
    //! these tests exist so refactors of the helpers and read-only ops
    //! can be validated without spinning up a full server.
    use super::*;
    use crate::db::test_pool;
    use serde_json::json;

    // ---- pure helper tests -------------------------------------------------

    #[test]
    fn test_parse_uuid_missing_key_returns_bad_args() {
        let err = parse_uuid(&json!({}), "id").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("missing 'id'"), "msg: {}", err.message);
    }

    #[test]
    fn test_parse_uuid_invalid_uuid_returns_bad_args() {
        let err = parse_uuid(&json!({"id": "not-a-uuid"}), "id").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.starts_with("bad 'id'"), "msg: {}", err.message);
    }

    #[test]
    fn test_parse_uuid_valid_returns_uuid() {
        let id = uuid::Uuid::new_v4();
        let parsed = parse_uuid(&json!({"id": id.to_string()}), "id").unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_parse_str_missing_returns_bad_args() {
        let err = parse_str(&json!({}), "tool").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("missing 'tool'"));
    }

    #[test]
    fn test_parse_str_empty_string_returns_bad_args() {
        let err = parse_str(&json!({"tool": ""}), "tool").unwrap_err();
        assert_eq!(err.code, "bad_args");
    }

    #[test]
    fn test_parse_str_valid_returns_borrowed_str() {
        let v = json!({"tool": "search"});
        assert_eq!(parse_str(&v, "tool").unwrap(), "search");
    }

    #[test]
    fn test_optional_str_missing_returns_none() {
        assert!(optional_str(&json!({}), "note").unwrap().is_none());
    }

    #[test]
    fn test_optional_str_null_returns_none() {
        assert!(optional_str(&json!({"note": null}), "note")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_optional_str_non_string_returns_bad_args() {
        let err = optional_str(&json!({"note": 7}), "note").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("'note' must be a string"));
    }

    #[test]
    fn test_optional_str_valid_returns_some() {
        let v = json!({"note": "hi"});
        assert_eq!(optional_str(&v, "note").unwrap(), Some("hi"));
    }

    #[test]
    fn test_actor_from_args_missing_field_defaults_to_user() {
        assert_eq!(actor_from_args(&json!({})), "user");
    }

    #[test]
    fn test_actor_from_args_explicit_user_passes_through() {
        assert_eq!(actor_from_args(&json!({"_actor": "user"})), "user");
    }

    #[test]
    fn test_actor_from_args_mcp_prefix_passes_through() {
        assert_eq!(
            actor_from_args(&json!({"_actor": "mcp:gmail-bot"})),
            "mcp:gmail-bot"
        );
    }

    #[test]
    fn test_actor_from_args_disallowed_actor_falls_back_to_user() {
        // Anything not "user" or "mcp:<name>" must not be honoured —
        // the audit-log actor field would otherwise be a free-form
        // injection sink.
        assert_eq!(actor_from_args(&json!({"_actor": "root"})), "user");
        assert_eq!(actor_from_args(&json!({"_actor": "mcp:"})), "user");
    }

    #[test]
    fn test_actor_is_allowed_rejects_overlong_mcp_name() {
        let long = format!("mcp:{}", "a".repeat(129));
        assert!(!actor_is_allowed(&long));
        let just_at_limit = format!("mcp:{}", "a".repeat(128));
        assert!(actor_is_allowed(&just_at_limit));
    }

    #[test]
    fn test_role_for_known_imap_names_maps_to_role() {
        assert_eq!(role_for("INBOX"), FolderRole::Inbox);
        assert_eq!(role_for("Sent"), FolderRole::Sent);
        assert_eq!(role_for("[Gmail]/Drafts"), FolderRole::Drafts);
        assert_eq!(role_for("[Gmail]/Trash"), FolderRole::Trash);
        assert_eq!(role_for("[Gmail]/All Mail"), FolderRole::All);
        assert_eq!(role_for("Spam"), FolderRole::Spam);
        assert_eq!(role_for("Starred"), FolderRole::Starred);
    }

    #[test]
    fn test_role_for_unknown_name_maps_to_custom() {
        assert_eq!(role_for("Receipts/2024"), FolderRole::Custom);
    }

    #[test]
    fn test_parse_addr_array_non_array_returns_bad_args() {
        let err = parse_addr_array(&json!("a@b.com"), "to_addrs").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("must be an array"));
    }

    #[test]
    fn test_parse_addr_array_blank_entry_returns_bad_args() {
        let err = parse_addr_array(&json!(["  "]), "to_addrs").unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("must not contain empty addresses"));
    }

    #[test]
    fn test_parse_addr_array_trims_and_returns_strings() {
        let parsed = parse_addr_array(&json!([" a@x.com ", "b@y.com"]), "to_addrs").unwrap();
        assert_eq!(parsed, vec!["a@x.com".to_string(), "b@y.com".to_string()]);
    }

    #[test]
    fn test_all_recipients_empty_returns_bad_args() {
        let err = all_recipients(&[], &[], &[]).unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("at least one recipient"));
    }

    #[test]
    fn test_all_recipients_concatenates_to_cc_bcc_in_order() {
        let to = vec!["t@x".to_string()];
        let cc = vec!["c@x".to_string()];
        let bcc = vec!["b@x".to_string()];
        assert_eq!(
            all_recipients(&to, &cc, &bcc).unwrap(),
            vec!["t@x".to_string(), "c@x".to_string(), "b@x".to_string()]
        );
    }

    #[test]
    fn test_pick_attachment_bytes_prefers_content_id_match() {
        use crate::mail::parser::{Disposition, ParsedAttachment};
        let parsed = vec![
            ParsedAttachment {
                filename: "wrong.txt".into(),
                content_type: "text/plain".into(),
                data: b"by-name".to_vec(),
                disposition: Disposition::Attachment,
                content_id: Some("cid-1".into()),
            },
            ParsedAttachment {
                filename: "logo.png".into(),
                content_type: "image/png".into(),
                data: b"by-cid".to_vec(),
                disposition: Disposition::Inline,
                content_id: Some("cid-target".into()),
            },
        ];
        let attachment = crate::models::Attachment {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: "logo.png".into(),
            content_type: "image/png".into(),
            content_id: Some("cid-target".into()),
            size_bytes: 6,
            disposition: crate::models::AttachmentDisposition::Inline,
            storage_path: String::new(),
            created_at: chrono::Utc::now(),
        };
        let bytes = pick_attachment_bytes(parsed, &attachment).unwrap();
        assert_eq!(bytes, b"by-cid");
    }

    #[test]
    fn test_pick_attachment_bytes_falls_back_to_filename_when_cid_absent() {
        use crate::mail::parser::{Disposition, ParsedAttachment};
        let parsed = vec![ParsedAttachment {
            filename: "report.pdf".into(),
            content_type: "application/pdf".into(),
            data: b"pdf-bytes".to_vec(),
            disposition: Disposition::Attachment,
            content_id: None,
        }];
        let attachment = crate::models::Attachment {
            id: AttachmentId::new(),
            message_id: MessageId::new(),
            filename: "report.pdf".into(),
            content_type: "application/pdf".into(),
            content_id: None,
            size_bytes: 9,
            disposition: crate::models::AttachmentDisposition::Attachment,
            storage_path: String::new(),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(
            pick_attachment_bytes(parsed, &attachment).unwrap(),
            b"pdf-bytes"
        );
    }

    #[test]
    fn test_map_smtp_error_auth_maps_to_auth_failed_code() {
        let err = map_smtp_error(SmtpError::Auth("nope".into()));
        assert_eq!(err.code, "auth_failed");
        assert_eq!(err.message, "nope");
    }

    #[test]
    fn test_map_smtp_error_invalid_request_maps_to_bad_args() {
        let err = map_smtp_error(SmtpError::InvalidRequest("missing to".into()));
        assert_eq!(err.code, "bad_args");
    }

    #[test]
    fn test_map_smtp_error_transient_maps_to_internal() {
        let err = map_smtp_error(SmtpError::Transient("retry later".into()));
        assert_eq!(err.code, "internal");
    }

    #[test]
    fn test_map_attachment_read_error_not_found_maps_to_bad_args() {
        let err = map_attachment_read_error(
            "/nope.txt",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("file not found"));
    }

    #[test]
    fn test_require_draft_attachment_content_missing_returns_internal() {
        let id = uuid::Uuid::new_v4();
        let err = require_draft_attachment_content(id, None).unwrap_err();
        assert_eq!(err.code, "internal");
        assert!(err.message.contains(&id.to_string()));
    }

    #[test]
    fn test_require_draft_attachment_content_allows_empty_bytes() {
        let bytes =
            require_draft_attachment_content(uuid::Uuid::new_v4(), Some(Vec::new())).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_map_attachment_export_error_already_exists_maps_to_bad_args() {
        let err =
            map_attachment_export_error(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
        assert_eq!(err.code, "bad_args");
    }

    #[test]
    fn test_map_attachment_export_error_other_maps_to_internal() {
        let err = map_attachment_export_error(std::io::Error::other("disk full"));
        assert_eq!(err.code, "internal");
    }

    // ---- DB-only op handler tests -----------------------------------------

    #[tokio::test]
    async fn test_op_account_list_empty_pool_returns_empty_array() {
        let pool = test_pool().await;
        let value = op_account_list(&pool).await.unwrap();
        assert_eq!(value, json!([]));
    }

    #[tokio::test]
    async fn test_op_audit_list_empty_pool_returns_empty_array() {
        let pool = test_pool().await;
        let value = op_audit_list(&pool, json!({})).await.unwrap();
        assert_eq!(value, json!([]));
    }

    #[tokio::test]
    async fn test_op_audit_list_after_record_returns_entry() {
        let pool = test_pool().await;
        // The audit() helper writes through the same `db::audit::record`
        // path the live ops use, so the read is exercised end-to-end.
        audit(&pool, "test.action", Some("target-1"), &json!({"k": 1})).await;
        let value = op_audit_list(&pool, json!({"limit": 10})).await.unwrap();
        let arr = value.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["action"], "test.action");
        assert_eq!(arr[0]["actor"], "user");
        assert_eq!(arr[0]["target"], "target-1");
    }

    #[tokio::test]
    async fn test_op_message_get_unknown_id_returns_null() {
        let pool = test_pool().await;
        let id = uuid::Uuid::new_v4();
        let value = op_message_get(&pool, json!({"id": id.to_string()}))
            .await
            .unwrap();
        assert!(value.is_null(), "expected null, got {value}");
    }

    #[tokio::test]
    async fn test_op_message_get_missing_id_returns_bad_args() {
        let pool = test_pool().await;
        let err = op_message_get(&pool, json!({})).await.unwrap_err();
        assert_eq!(err.code, "bad_args");
    }

    #[tokio::test]
    async fn test_op_search_empty_query_returns_bad_args() {
        let pool = test_pool().await;
        let err = op_search(&pool, json!({"q": ""})).await.unwrap_err();
        assert_eq!(err.code, "bad_args");
    }

    #[tokio::test]
    async fn test_op_search_missing_query_returns_bad_args() {
        let pool = test_pool().await;
        let err = op_search(&pool, json!({})).await.unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("missing 'q'"));
    }

    #[tokio::test]
    async fn test_op_sql_query_with_select_returns_rows() {
        let pool = test_pool().await;
        // In-memory SQLite has no separate RO connection; the keyword
        // scan is the actual safety check being tested here.
        let read_pool = pool.clone();
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO accounts \
             (id, email, display_name, auth_kind, secret_ref, \
              imap_host, imap_port, imap_use_tls, smtp_host, smtp_port, \
              smtp_use_tls, smtp_starttls, created_at) \
             VALUES (?, 'a@b.com', 'A', 'password', NULL, \
                     'imap.example', 993, 1, 'smtp.example', 587, 0, 1, '2026-01-01T00:00:00Z')",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
        let value = op_sql_query(&read_pool, json!({"sql": "SELECT email FROM accounts"}))
            .await
            .unwrap();
        let rows = value.as_array().expect("array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["email"], "a@b.com");
    }

    #[tokio::test]
    async fn test_op_sql_query_with_insert_returns_bad_args() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let err = op_sql_query(
            &read_pool,
            json!({"sql": "INSERT INTO accounts(id) VALUES ('x')"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(
            err.message.contains("INSERT"),
            "expected mention of INSERT, got: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn test_op_sql_query_missing_sql_returns_bad_args() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let err = op_sql_query(&read_pool, json!({})).await.unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("missing 'sql'"));
    }

    #[tokio::test]
    async fn test_op_sql_query_default_limit_returns_default_rows() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let value = op_sql_query(
            &read_pool,
            json!({
                "sql": "WITH RECURSIVE n(x) AS ( \
                            VALUES(1) \
                            UNION ALL \
                            SELECT x + 1 FROM n WHERE x < 250 \
                        ) \
                        SELECT x FROM n"
            }),
        )
        .await
        .unwrap();
        let rows = value.as_array().expect("array");
        assert_eq!(rows.len(), crate::db::sql_query::DEFAULT_ROWS);
    }

    #[tokio::test]
    async fn test_op_sql_query_limit_over_max_is_capped() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let value = op_sql_query(
            &read_pool,
            json!({
                "sql": "WITH RECURSIVE n(x) AS ( \
                            VALUES(1) \
                            UNION ALL \
                            SELECT x + 1 FROM n WHERE x < 1005 \
                        ) \
                        SELECT x FROM n",
                "limit": 2000,
            }),
        )
        .await
        .unwrap();
        let rows = value.as_array().expect("array");
        assert_eq!(rows.len(), crate::db::sql_query::MAX_ROWS);
    }

    #[tokio::test]
    async fn test_op_sql_query_invalid_limit_returns_bad_args() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let err = op_sql_query(&read_pool, json!({"sql": "SELECT 1", "limit": 0}))
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_args");
        assert!(err.message.contains("limit"));
    }

    #[tokio::test]
    async fn test_op_sql_schema_returns_table_definitions() {
        let pool = test_pool().await;
        let read_pool = pool.clone();
        let value = op_sql_schema(&read_pool).await.unwrap();
        let rows = value.as_array().expect("array");
        let names: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("name").and_then(Value::as_str).map(String::from))
            .collect();
        assert!(
            names.iter().any(|n| n == "accounts"),
            "expected 'accounts' table in schema dump, got: {names:?}"
        );
        assert!(names.iter().any(|n| n == "messages"));
    }

    #[tokio::test]
    async fn test_op_mcp_gate_list_empty_pool_returns_empty_array() {
        let pool = test_pool().await;
        let value = op_mcp_gate_list(&pool, json!({})).await.unwrap();
        assert_eq!(value, json!([]));
    }

    #[tokio::test]
    async fn test_op_mcp_approval_list_empty_pool_returns_empty_array() {
        let pool = test_pool().await;
        let value = op_mcp_approval_list(&pool, json!({})).await.unwrap();
        assert_eq!(value, json!([]));
    }
}
