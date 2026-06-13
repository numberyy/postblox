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

use std::{collections::HashMap, sync::Arc};

use chrono::Utc;
use serde_json::json;
use sqlx::SqlitePool;

use crate::auth::MailCredential;
use crate::db;
use crate::imap::ImapSync;
use crate::ipc::{Hub, Topic};
use crate::mail::parser::{parse_with_options, ParseOptions, ParsedEmail};
use crate::mail::threading::{assign_thread, ThreadMatch, ThreadRef};
use crate::models::{AccountId, AddressList, Folder, FolderId, Message, MessageFlags, ThreadId};

use super::error::SyncError;

/// What `reconcile_folder` returns. `inserted` counts new messages
/// added to SQLite. `wiped` is set when UIDVALIDITY changed and we had
/// to refetch from scratch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReconcileReport {
    /// Folder that was reconciled.
    pub folder_id: FolderId,
    /// Number of messages newly inserted into the local store.
    pub inserted: u64,
    /// Number of messages wiped because `UIDVALIDITY` changed.
    pub wiped: u64,
    /// Server-reported `UIDVALIDITY` after reconciliation, if any.
    pub uid_validity: Option<i64>,
    /// Server-reported `UIDNEXT` after reconciliation, if any.
    pub uid_next: Option<i64>,
    /// Highest UID the worker has now ingested for the folder, if any.
    pub last_seen_uid: Option<i64>,
}

/// Pull new IMAP messages for `folder_name` and write them to the
/// local SQLite store, returning a summary of what changed.
///
/// # Errors
///
/// Returns:
/// - [`SyncError::MissingCredentials`] if `credential` is empty.
/// - [`SyncError::UnknownAccount`] if `account_id` does not exist locally.
/// - [`SyncError::UnknownFolder`] if `folder_name` does not exist for the
///   account.
/// - [`SyncError::Imap`] wrapping any [`crate::imap::ImapError`] surfaced
///   by [`ImapSync::sync_folder`] (auth, network, protocol).
/// - [`SyncError::Db`] if any SQLite read or write fails (folder lookup,
///   thread/message insert, UID-state update).
/// - [`SyncError::Attachment`] if persisting a fetched attachment fails;
///   the half-inserted message is best-effort rolled back.
///
/// Unparseable messages are skipped with a warning rather than failing
/// the whole sync.
pub async fn reconcile_folder(
    pool: &SqlitePool,
    hub: &Arc<Hub>,
    imap: &dyn ImapSync,
    account_id: AccountId,
    folder_name: &str,
    credential: &MailCredential,
) -> Result<ReconcileReport, SyncError> {
    if credential.is_empty() {
        return Err(SyncError::MissingCredentials);
    }
    let account = db::accounts::get(pool, account_id)
        .await?
        .ok_or(SyncError::UnknownAccount)?;
    let folder: Folder = db::folders::get_by_name(pool, account_id, folder_name)
        .await?
        .ok_or_else(|| SyncError::UnknownFolder(folder_name.to_string()))?;

    let port = crate::imap::port_u16(account.imap_port)?;
    // IMAP UIDs are u32; clamp the stored i64 before the +1 so a corrupt
    // oversized value can't wrap to a low UID and silently re-fetch.
    let mut from_uid = (folder
        .last_seen_uid
        .unwrap_or(0)
        .clamp(0, i64::from(u32::MAX)) as u32)
        .saturating_add(1);
    let mut server = imap
        .sync_folder(
            &account.imap_host,
            port,
            &account.email,
            credential,
            folder_name,
            from_uid,
        )
        .await?;

    // UIDVALIDITY changed under us: wipe everything we have for this
    // folder and refetch from UID 1.
    let mut wiped: u64 = 0;
    let needs_full_resync = match (folder.uid_validity, server.uid_validity) {
        (Some(local), Some(server_v)) => local != i64::from(server_v),
        _ => false,
    };
    if needs_full_resync {
        wiped = db::messages::delete_all_in_folder(pool, folder.id).await?;
        from_uid = 1;
        // Fetch from scratch.
        server = imap
            .sync_folder(
                &account.imap_host,
                port,
                &account.email,
                credential,
                folder_name,
                from_uid,
            )
            .await?;
    }

    // Pull recent threads once so the thread-matcher has somewhere to
    // look for In-Reply-To / References / subject hits.
    let recent = db::threads::list_recent(pool, account_id, 200, 0).await?;
    let recent_thread_ids: Vec<ThreadId> = recent.iter().map(|thread| thread.id).collect();
    let mut message_ids_by_thread: HashMap<ThreadId, Vec<String>> =
        HashMap::with_capacity(recent_thread_ids.len());
    for (thread_id, message_id) in
        db::messages::message_ids_by_threads(pool, &recent_thread_ids).await?
    {
        message_ids_by_thread
            .entry(thread_id)
            .or_default()
            .push(message_id);
    }
    let mut thread_refs: Vec<ThreadRef> = Vec::with_capacity(recent.len());
    for t in &recent {
        thread_refs.push(ThreadRef {
            thread_id: t.id.into_inner(),
            message_ids: message_ids_by_thread.remove(&t.id).unwrap_or_default(),
            subject: t.subject.clone().unwrap_or_default(),
            last_message_at: t.last_message_at.unwrap_or_else(Utc::now),
        });
    }

    let mut inserted: u64 = 0;
    // On a UIDVALIDITY change we wiped the folder and refetch from UID 1, so
    // the server reassigned UIDs from scratch — the old high-water mark must
    // NOT carry forward into the running max, or it would permanently skip
    // the reassigned low-UID messages. Reset the baseline in that case.
    let mut last_seen_uid = if needs_full_resync {
        None
    } else {
        folder.last_seen_uid
    };
    // Drain the folder in bounded windows: each `sync_folder` returns at
    // most FETCH_WINDOW message bodies, so peak memory stays bounded even
    // on a first sync of a large mailbox. `has_more` advances us to the
    // next window without waiting for the next poll/IDLE cycle.
    loop {
        // Skip messages we already have. The server may include the
        // boundary UID even within a window.
        let server_uids: Vec<i64> = server.messages.iter().map(|m| m.uid as i64).collect();
        let already = db::messages::existing_uids(pool, folder.id, &server_uids).await?;
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
            let mut parsed: ParsedEmail =
                match parse_with_options(&fetched.raw, ParseOptions::without_raw_headers()) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(uid, error = %e, "skip unparseable message");
                        continue;
                    }
                };

            let thread_id = match assign_thread(&parsed, &thread_refs) {
                ThreadMatch::Existing(id) => ThreadId::from(id),
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
                        thread_id: t.id.into_inner(),
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

            // Take attachments out before moving `parsed` into `build_message_row` so
            // we can persist them afterwards without re-borrowing a moved value.
            let attachments = std::mem::take(&mut parsed.attachments);
            let new = build_message_row(account_id, folder.id, thread_id, fetched, parsed);
            let row: Message = db::messages::create(pool, &new).await?;
            if let Err(error) =
                crate::attachments::persist_parsed_for_message(pool, row.id, &attachments).await
            {
                // best-effort rollback of the half-inserted message; original error takes priority.
                let _ = db::messages::delete(pool, row.id).await;
                return Err(error.into());
            }
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

        // Advance bookkeeping for this window, then fetch the next batch
        // if the server reported UIDs above it.
        let batch_max = server.messages.iter().map(|m| m.uid as i64).max();
        last_seen_uid = [last_seen_uid, batch_max, Some(i64::from(server.window_hi))]
            .into_iter()
            .flatten()
            .max();
        if !server.has_more {
            break;
        }
        from_uid = server.window_hi.saturating_add(1);
        server = imap
            .sync_folder(
                &account.imap_host,
                port,
                &account.email,
                credential,
                folder_name,
                from_uid,
            )
            .await?;
    }

    db::folders::update_uid_state(
        pool,
        folder.id,
        server.uid_validity.map(|v| v as i64),
        server.uid_next.map(|v| v as i64),
        last_seen_uid,
    )
    .await?;

    // Record a successful sync so the status survives a restart and the
    // accounts pane can show "last synced": clears any prior error and
    // stamps `last_synced_at`.
    db::accounts::update_sync(
        pool,
        account_id,
        crate::models::SyncStatus::Idle,
        None,
        Some(Utc::now()),
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
    account_id: AccountId,
    folder_id: FolderId,
    thread_id: ThreadId,
    fetched: &crate::imap::FetchedMessage,
    parsed: ParsedEmail,
) -> db::messages::NewMessage {
    let snippet = parsed.text_body.as_deref().map(|t| {
        let mut out = String::with_capacity(200);
        for ch in t.chars().take(200) {
            out.push(if matches!(ch, '\n' | '\r') { ' ' } else { ch });
        }
        out
    });
    let references_header = if parsed.references.is_empty() {
        None
    } else {
        Some(parsed.references.join(" "))
    };
    db::messages::NewMessage {
        account_id,
        folder_id,
        thread_id: Some(thread_id),
        uid: fetched.uid as i64,
        message_id_header: parsed.message_id,
        in_reply_to: parsed.in_reply_to,
        references_header,
        from_addr: parsed.from,
        to_addrs: AddressList::from(parsed.to),
        cc_addrs: AddressList::from(parsed.cc),
        bcc_addrs: AddressList::default(),
        reply_to: None,
        subject: parsed.subject,
        snippet,
        text_body: parsed.text_body,
        html_body: parsed.html_body,
        raw_size: fetched.raw.len() as i64,
        flags: MessageFlags::from(fetched.flags.clone()),
        internal_date: fetched.internal_date.unwrap_or_else(Utc::now),
        sent_at: None,
    }
}
