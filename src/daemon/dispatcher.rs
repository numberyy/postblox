//! `DaemonDispatcher` — the concrete `ipc::Dispatcher` impl.
//!
//! Maps wire op names to `db::*` calls and publishes events on the
//! [`Hub`] for write ops. No IMAP/SMTP yet — that wires in R3b.

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::auth::MailCredential;
use crate::db;
use crate::imap::{self, ImapAuth, ImapIdle, ImapSync};
use crate::ipc::{Dispatcher, Hub, RpcError, Topic};
use crate::models::{Account, ApprovalState, AuthKind, FolderRole, GateAction};
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
    async fn resolve(&self, account_id: uuid::Uuid) -> Result<MailCredential, sync::SyncError> {
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
    hub: Arc<Hub>,
    imap: Arc<dyn ImapAuth>,
    imap_sync: Arc<dyn ImapSync>,
    secrets: Arc<dyn SecretStore>,
    oauth: Arc<dyn GoogleOAuth>,
    smtp: Arc<dyn SmtpSubmitter>,
    worker_manager: Arc<sync::WorkerManager>,
}

impl DaemonDispatcher {
    /// Production constructor: TLS-backed IMAP via rustls.
    pub fn new(pool: SqlitePool, hub: Arc<Hub>, secrets: Arc<dyn SecretStore>) -> Self {
        let imap = imap::default_auth().expect("rustls platform verifier init");
        let imap_sync = imap::default_sync().expect("rustls platform verifier init");
        let imap_idle = imap::default_idle().expect("rustls platform verifier init");
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
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
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
        Self::with_imap_smtp_oauth_and_manager(
            pool,
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
        Self::with_imap_smtp_oauth_and_manager(
            pool,
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
        Self::with_imap_smtp_oauth_and_manager(
            pool,
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
        Self::with_imap_smtp_oauth_and_manager(
            pool,
            hub,
            imap,
            imap_sync,
            secrets,
            services,
            worker_manager,
        )
    }

    pub fn with_imap_smtp_oauth_and_manager(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapAuth>,
        imap_sync: Arc<dyn ImapSync>,
        secrets: Arc<dyn SecretStore>,
        services: DaemonServices,
        worker_manager: Arc<sync::WorkerManager>,
    ) -> Self {
        Self {
            pool,
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
    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError> {
        match op {
            // -- read ops --
            "account.list" => op_account_list(&self.pool).await,
            "folder.list" => op_folder_list(&self.pool, args).await,
            "thread.list" => op_thread_list(&self.pool, args).await,
            "message.list_by_folder" => op_messages_by_folder(&self.pool, args).await,
            "message.list_by_thread" => op_messages_by_thread(&self.pool, args).await,
            "message.get" => op_message_get(&self.pool, args).await,
            "attachment.list" => op_attachment_list(&self.pool, args).await,
            "attachment.preview" => op_attachment_preview(&self.pool, args).await,
            "search" => op_search(&self.pool, args).await,
            "audit.list_recent" => op_audit_list(&self.pool, args).await,

            // -- MCP gate/approval ops --
            "mcp.gate.list" => op_mcp_gate_list(&self.pool, args).await,
            "mcp.gate.create" => op_mcp_gate_create(&self.pool, args).await,
            "mcp.gate.delete" => op_mcp_gate_delete(&self.pool, args).await,
            "mcp.approval.create" => op_mcp_approval_create(&self.pool, &self.hub, args).await,
            "mcp.approval.list" => op_mcp_approval_list(&self.pool, args).await,
            "mcp.approval.get" => op_mcp_approval_get(&self.pool, args).await,
            "mcp.approval.decide" => op_mcp_approval_decide(&self.pool, &self.hub, args).await,

            // -- write ops --
            "account.create" => op_account_create(&self.pool, args).await,
            "account.delete" => op_account_delete(&self.pool, args).await,
            "folder.upsert" => op_folder_upsert(&self.pool, args).await,
            "message.set_flags" => op_message_set_flags(&self.pool, &self.hub, args).await,
            "draft.create" => op_draft_create(&self.pool, args).await,
            "draft.update" => op_draft_update(&self.pool, args).await,
            "draft.delete" => op_draft_delete(&self.pool, args).await,
            "attachment.export" => op_attachment_export(&self.pool, args).await,
            "message.send" => {
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
            "account.test_login" => {
                op_account_test_login(
                    &self.pool,
                    self.imap.as_ref(),
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
            }
            "account.sync_folder" => {
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
            "account.start_sync" => {
                op_account_start_sync(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
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

            // -- OAuth ops --
            "oauth.google.auth_url" => op_oauth_google_auth_url(&self.pool, args).await,
            "oauth.google.complete" => {
                op_oauth_google_complete(
                    &self.pool,
                    self.secrets.as_ref(),
                    self.oauth.as_ref(),
                    args,
                )
                .await
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

async fn op_attachment_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let message_id = parse_uuid(&args, "message_id")?;
    encode(
        db::attachments::list_for_message(pool, message_id).await,
        "attachments::list_for_message",
    )
}

async fn op_attachment_preview(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
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
    let actor = actor_from_args(&args);
    let id = parse_uuid(&args, "id")?;
    let flags = args
        .get("flags")
        .cloned()
        .ok_or_else(|| RpcError::bad_args("missing 'flags'"))?;
    db::messages::set_flags(pool, id, &flags)
        .await
        .map_err(|e| RpcError::internal(format!("messages::set_flags: {e}")))?;
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
    Ok(json!({"ok": true}))
}

async fn op_draft_create(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let new: db::drafts::NewDraft =
        serde_json::from_value(args).map_err(|e| RpcError::bad_args(e.to_string()))?;
    let draft = db::drafts::create(pool, &new)
        .await
        .map_err(|e| RpcError::internal(format!("drafts::create: {e}")))?;
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
    let actor = actor_from_args(&args);
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

async fn op_draft_delete(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_uuid(&args, "id")?;
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

async fn op_attachment_export(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let actor = actor_from_args(&args);
    let id = parse_uuid(&args, "id")?;
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
    let account_id = parse_uuid(&args, "account_id")?;
    let draft_id = parse_uuid(&args, "draft_id")?;

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
    let subject = draft.subject.clone().unwrap_or_default();
    let mime = crate::mail::builder::build_mime(
        &account.email,
        &to,
        &cc,
        &subject,
        draft.text_body.as_deref(),
        draft.html_body.as_deref(),
        &message_id,
    );

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

    audit_actor(
        pool,
        &actor,
        "message.send",
        Some(&draft_id.to_string()),
        &json!({
            "account_id": account_id,
            "message_id": message_id,
        }),
    )
    .await;
    Ok(json!({"ok": true, "message_id": message_id}))
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
    let account_id = parse_uuid(&args, "account_id")?;

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
    oauth: &dyn GoogleOAuth,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let folder_name = args
        .get("folder_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'folder_name'"))?;

    let account = db::accounts::get(pool, account_id)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::get: {e}")))?
        .ok_or_else(|| RpcError::bad_args("unknown account_id"))?;
    let credential = credential_for_account(secrets, oauth, &account).await?;

    let report = sync::reconcile_folder(pool, hub, imap_sync, account_id, folder_name, &credential)
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
    oauth: &dyn GoogleOAuth,
    manager: &sync::WorkerManager,
    args: Value,
) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
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
    let account_id = parse_uuid(&args, "account_id")?;
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
    let account_id = parse_uuid(&args, "account_id")?;
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
    let account_id = parse_uuid(&args, "account_id")?;
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
    account_id: uuid::Uuid,
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
