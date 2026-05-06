//! Reconcile a single IMAP folder with the local SQLite store.
//!
//! Steps:
//!   1. Look up the local folder row and its UID state.
//!   2. Call `ImapSync::sync_folder` from `last_seen_uid + 1` to `*`.
//!   3. If the server's `UIDVALIDITY` differs from ours, wipe the
//!      folder's local messages and refetch from UID 1.
//!   4. For each fetched message: skip if we already have its UID,
//!      otherwise parse, thread-assign, insert, and publish
//!      `mail.new`.
//!   5. Update the folder's `uid_validity` / `uid_next` /
//!      `last_seen_uid` and `account.synced` once.
//!
//! The function takes a `&dyn ImapSync` so tests can substitute a mock
//! that returns canned bytes — we never open a real network connection
//! during `cargo test`.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db;
use crate::imap::ImapSync;
use crate::ipc::{Hub, Topic};
use crate::mail::parser::{parse, ParsedEmail};
use crate::mail::threading::{assign_thread, ThreadMatch, ThreadRef};
use crate::models::{Folder, Message};

use super::error::SyncError;

/// What `reconcile_folder` returns. `inserted` counts new messages
/// added to SQLite. `wiped` is set when UIDVALIDITY changed and we had
/// to refetch from scratch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconcileReport {
    pub folder_id: Uuid,
    pub inserted: u64,
    pub wiped: u64,
    pub uid_validity: Option<i64>,
    pub uid_next: Option<i64>,
    pub last_seen_uid: Option<i64>,
}

pub async fn reconcile_folder(
    pool: &SqlitePool,
    hub: &Arc<Hub>,
    imap: &dyn ImapSync,
    account_id: Uuid,
    folder_name: &str,
    password: &str,
) -> Result<ReconcileReport, SyncError> {
    if password.is_empty() {
        return Err(SyncError::MissingCredentials);
    }
    let account = db::accounts::get(pool, account_id)
        .await?
        .ok_or(SyncError::UnknownAccount)?;
    let folder: Folder = db::folders::get_by_name(pool, account_id, folder_name)
        .await?
        .ok_or_else(|| SyncError::UnknownFolder(folder_name.to_string()))?;

    let from_uid = folder.last_seen_uid.unwrap_or(0).max(0) as u32 + 1;
    let server = imap
        .sync_folder(
            &account.imap_host,
            account.imap_port as u16,
            &account.email,
            password,
            folder_name,
            from_uid,
        )
        .await?;

    // UIDVALIDITY changed under us: wipe everything we have for this
    // folder and refetch from UID 1.
    let mut wiped: u64 = 0;
    let needs_full_resync = match (folder.uid_validity, server.uid_validity) {
        (Some(local), Some(server_v)) => local != server_v as i64,
        _ => false,
    };
    let server = if needs_full_resync {
        wiped = db::messages::delete_all_in_folder(pool, folder.id).await?;
        // Fetch from scratch.
        imap.sync_folder(
            &account.imap_host,
            account.imap_port as u16,
            &account.email,
            password,
            folder_name,
            1,
        )
        .await?
    } else {
        server
    };

    // Skip messages we already have. The server may include the
    // boundary UID `last_seen_uid` even with `<from>:*` semantics.
    let server_uids: Vec<i64> = server.messages.iter().map(|m| m.uid as i64).collect();
    let already = db::messages::existing_uids(pool, folder.id, &server_uids).await?;

    // Pull recent threads once so the thread-matcher has somewhere to
    // look for In-Reply-To / References / subject hits.
    let recent = db::threads::list_recent(pool, account_id, 200, 0).await?;
    let mut thread_refs: Vec<ThreadRef> = Vec::with_capacity(recent.len());
    for t in &recent {
        let ids = db::messages::list_by_thread(pool, t.id)
            .await?
            .into_iter()
            .filter_map(|m| m.message_id_header)
            .collect::<Vec<_>>();
        thread_refs.push(ThreadRef {
            thread_id: t.id,
            message_ids: ids,
            subject: t.subject.clone().unwrap_or_default(),
            last_message_at: t.last_message_at.unwrap_or_else(Utc::now),
        });
    }

    let mut inserted: u64 = 0;
    for fetched in &server.messages {
        let uid = fetched.uid as i64;
        if already.contains(&uid) {
            continue;
        }
        // Some servers may legitimately not include a body for some UIDs
        // (e.g. expunged race). Skip rather than fail the whole sync.
        if fetched.raw.is_empty() {
            continue;
        }
        let parsed: ParsedEmail = match parse(&fetched.raw) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(uid, error = %e, "skip unparseable message");
                continue;
            }
        };

        let thread_id = match assign_thread(&parsed, &thread_refs) {
            ThreadMatch::Existing(id) => id,
            ThreadMatch::New => {
                let t = db::threads::create(
                    pool,
                    account_id,
                    parsed.message_id.as_deref(),
                    parsed.subject.as_deref(),
                )
                .await?;
                // Add to the in-memory list so subsequent messages in
                // this same batch can match against it.
                thread_refs.push(ThreadRef {
                    thread_id: t.id,
                    message_ids: parsed
                        .message_id
                        .as_ref()
                        .map(|m| vec![m.clone()])
                        .unwrap_or_default(),
                    subject: parsed.subject.clone().unwrap_or_default(),
                    last_message_at: fetched.internal_date.unwrap_or_else(Utc::now),
                });
                t.id
            }
        };

        let new = build_message_row(account_id, folder.id, thread_id, fetched, &parsed);
        let row: Message = db::messages::create(pool, &new).await?;
        db::threads::touch_last_message_at(
            pool,
            thread_id,
            fetched.internal_date.unwrap_or_else(Utc::now),
        )
        .await?;
        db::threads::refresh_aggregates(pool, thread_id).await?;
        inserted += 1;

        hub.publish(
            Topic::MailNew,
            json!({
                "account_id": account_id,
                "folder_id": folder.id,
                "thread_id": thread_id,
                "message_id": row.id,
                "uid": uid,
            }),
        )
        .await;
    }

    let last_seen_uid = server
        .messages
        .iter()
        .map(|m| m.uid as i64)
        .max()
        .or(folder.last_seen_uid);
    db::folders::update_uid_state(
        pool,
        folder.id,
        server.uid_validity.map(|v| v as i64),
        server.uid_next.map(|v| v as i64),
        last_seen_uid,
    )
    .await?;

    if inserted > 0 || wiped > 0 || needs_full_resync {
        hub.publish(
            Topic::AccountSynced,
            json!({
                "account_id": account_id,
                "folder_id": folder.id,
                "inserted": inserted,
                "wiped": wiped,
            }),
        )
        .await;
    }

    Ok(ReconcileReport {
        folder_id: folder.id,
        inserted,
        wiped,
        uid_validity: server.uid_validity.map(|v| v as i64),
        uid_next: server.uid_next.map(|v| v as i64),
        last_seen_uid,
    })
}

/// Translate a parsed email + IMAP fetch into the row shape the db
/// layer wants. Pulled out as a small helper because the field list is
/// long and the test below is easier to read this way.
fn build_message_row(
    account_id: Uuid,
    folder_id: Uuid,
    thread_id: Uuid,
    fetched: &crate::imap::FetchedMessage,
    parsed: &ParsedEmail,
) -> db::messages::NewMessage {
    let snippet = parsed.text_body.as_deref().map(|t| {
        let trimmed: String = t.chars().take(200).collect();
        trimmed.replace(['\n', '\r'], " ")
    });
    db::messages::NewMessage {
        account_id,
        folder_id,
        thread_id: Some(thread_id),
        uid: fetched.uid as i64,
        message_id_header: parsed.message_id.clone(),
        in_reply_to: parsed.in_reply_to.clone(),
        references_header: if parsed.references.is_empty() {
            None
        } else {
            Some(parsed.references.join(" "))
        },
        from_addr: parsed.from.clone(),
        to_addrs: Value::Array(parsed.to.iter().cloned().map(Value::String).collect()),
        cc_addrs: Value::Array(parsed.cc.iter().cloned().map(Value::String).collect()),
        bcc_addrs: Value::Array(vec![]),
        reply_to: None,
        subject: parsed.subject.clone(),
        snippet,
        text_body: parsed.text_body.clone(),
        html_body: parsed.html_body.clone(),
        raw_size: fetched.raw.len() as i64,
        flags: Value::Array(fetched.flags.iter().cloned().map(Value::String).collect()),
        internal_date: fetched.internal_date.unwrap_or_else(Utc::now),
        sent_at: None,
    }
}
