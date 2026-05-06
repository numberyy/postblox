//! `postbloxd` — the local daemon.
//!
//! Single-user, single-process. Owns the SQLite pool and (later) the
//! IMAP IDLE workers. Speaks the IPC protocol over a Unix socket.
//!
//! R2 scope: read-only ops backed by the existing db layer. Write
//! ops + IMAP sync land in R3/R4.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::signal;

use postblox::db;
use postblox::ipc::{default_socket_path, listen, Dispatcher, Hub, RpcError};

#[derive(Clone)]
struct DaemonDispatcher {
    pool: SqlitePool,
}

#[async_trait::async_trait]
impl Dispatcher for DaemonDispatcher {
    async fn dispatch(&self, op: &str, args: Value) -> Result<Value, RpcError> {
        match op {
            "account.list" => op_account_list(&self.pool).await,
            "folder.list" => op_folder_list(&self.pool, args).await,
            "thread.list" => op_thread_list(&self.pool, args).await,
            "message.list_by_folder" => op_messages_by_folder(&self.pool, args).await,
            "message.list_by_thread" => op_messages_by_thread(&self.pool, args).await,
            "message.get" => op_message_get(&self.pool, args).await,
            "search" => op_search(&self.pool, args).await,
            other => Err(RpcError::unknown_op(other)),
        }
    }
}

async fn op_account_list(pool: &SqlitePool) -> Result<Value, RpcError> {
    let rows = db::accounts::list(pool)
        .await
        .map_err(|e| RpcError::internal(format!("accounts::list: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_folder_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "account_id")?;
    let rows = db::folders::list_by_account(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("folders::list: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_thread_list(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let account_id = parse_uuid(&args, "account_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    let rows = db::threads::list_recent(pool, account_id, limit, offset)
        .await
        .map_err(|e| RpcError::internal(format!("threads::list_recent: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_messages_by_folder(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let folder_id = parse_uuid(&args, "folder_id")?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    let rows = db::messages::list_by_folder(pool, folder_id, limit, offset)
        .await
        .map_err(|e| RpcError::internal(format!("messages::list_by_folder: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_messages_by_thread(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let thread_id = parse_uuid(&args, "thread_id")?;
    let rows = db::messages::list_by_thread(pool, thread_id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::list_by_thread: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_message_get(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let id = parse_uuid(&args, "id")?;
    let row = db::messages::get(pool, id)
        .await
        .map_err(|e| RpcError::internal(format!("messages::get: {e}")))?;
    serde_json::to_value(row).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

async fn op_search(pool: &SqlitePool, args: Value) -> Result<Value, RpcError> {
    let q = args
        .get("q")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args("missing 'q'"))?;
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0);
    let rows = db::search::search(pool, &db::search::quote_term(q), limit, offset)
        .await
        .map_err(|e| RpcError::internal(format!("search: {e}")))?;
    serde_json::to_value(rows).map_err(|e| RpcError::internal(format!("encode: {e}")))
}

fn parse_uuid(args: &Value, key: &str) -> Result<uuid::Uuid, RpcError> {
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::bad_args(format!("missing '{key}'")))?;
    uuid::Uuid::parse_str(s).map_err(|e| RpcError::bad_args(format!("bad '{key}': {e}")))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let socket_path = std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);
    let db_path = std::env::var_os("POSTBLOX_DB")
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path);

    tracing::info!(?db_path, ?socket_path, "starting postbloxd");

    let pool = db::connect(&db_path)
        .await
        .with_context(|| format!("connect to db at {}", db_path.display()))?;

    let hub = Arc::new(Hub::new());
    let dispatcher = Arc::new(DaemonDispatcher { pool });
    let server = listen(&socket_path, dispatcher, hub).await?;
    tracing::info!(socket = %server.path().display(), "listening");

    // Wait for ctrl-c, then shut down cleanly.
    signal::ctrl_c().await.context("install ctrl-c handler")?;
    tracing::info!("shutdown signal received");
    server.shutdown().await;
    Ok(())
}

fn default_db_path() -> PathBuf {
    if let Some(home) = dirs::data_local_dir() {
        home.join("postblox").join("postblox.db")
    } else {
        PathBuf::from("postblox.db")
    }
}
