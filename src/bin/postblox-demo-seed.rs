//! `postblox-demo-seed` — populates a local `postbloxd` with demo data.
//!
//! Drives a running daemon over its Unix socket to create accounts,
//! folders, drafts, MCP gates, and pending approvals. Messages are
//! inserted **directly into SQLite** via [`postblox::db::messages`] /
//! [`postblox::db::threads`] because the IPC surface deliberately does
//! not expose a generic `message.insert` op — message rows normally
//! arrive through the IMAP reconciler. Adding a new wire op just for
//! the seed would violate `CLAUDE.md`'s "No abstractions before the
//! third use" rule, so the seed opens its own short-lived
//! [`sqlx::SqlitePool`] against `POSTBLOX_DB` (in addition to the IPC
//! client) and writes through the same `db::*` helpers the daemon
//! uses internally. TODO: replace with a dedicated `message.upsert_raw`
//! op once a second caller materialises.
//!
//! Re-running the binary against an already-seeded DB is a no-op: the
//! account / folder / thread / draft / gate / approval lookups all
//! short-circuit on a stable natural key (email address, folder name,
//! thread external_id, draft subject, gate tool+pattern, approval
//! summary). Counts therefore stay constant across reseeds.
//!
//! Usage:
//! ```sh
//! POSTBLOX_SOCKET=/tmp/postblox-demo/postblox.sock \
//! POSTBLOX_DB=/tmp/postblox-demo/postblox.db \
//!     target/release/postblox-demo-seed
//! ```

#![deny(clippy::correctness)]
#![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::SqlitePool;

use postblox::db;
use postblox::ipc::client::Client;
use postblox::models::{AccountId, AddressList, FolderId, FolderRole, MessageFlags, ThreadId};

/// One demo account with the folder layout we populate per seed run.
struct DemoAccount {
    email: &'static str,
    display_name: &'static str,
    /// Per-account thread "topics" — each topic seeds 3 INBOX messages
    /// chained as a thread, so `topics.len() * 3 = INBOX message count`.
    topics: &'static [&'static str],
}

const DEMO_ACCOUNTS: &[DemoAccount] = &[
    DemoAccount {
        email: "alice@demo.local",
        display_name: "Alice Example",
        topics: &[
            "Sprint planning notes",
            "Quarterly review draft",
            "Lunch on Friday",
            "Conference travel",
            "Library book overdue",
        ],
    },
    DemoAccount {
        email: "team@demo.local",
        display_name: "Team Mailbox",
        topics: &[
            "Incident postmortem",
            "Hiring loop schedule",
            "Onboarding checklist",
            "Roadmap review",
            "Vendor renewal",
        ],
    },
    DemoAccount {
        email: "support@demo.local",
        display_name: "Support Inbox",
        topics: &[
            "Ticket #1024 follow-up",
            "Refund request",
            "Feature request: dark mode",
            "Login issues",
            "Mobile crash report",
        ],
    },
];

/// Number of messages chained inside each INBOX thread. With 5 topics
/// per account, this gives 30 INBOX messages — the per-account floor in
/// the seed spec.
const INBOX_MESSAGES_PER_THREAD: usize = 6;
/// Number of independent Sent rows seeded per account.
const SENT_MESSAGES_PER_ACCOUNT: usize = 3;
/// Number of independent Archive rows seeded per account.
const ARCHIVE_MESSAGES_PER_ACCOUNT: usize = 2;

const DEMO_FOLDERS: &[(&str, FolderRole)] = &[
    ("INBOX", FolderRole::Inbox),
    ("Sent", FolderRole::Sent),
    ("Drafts", FolderRole::Drafts),
    ("Archive", FolderRole::Archive),
];

/// Raw RFC822 corpus used to seed message bodies. Compile-time embedded
/// from `tests/fixtures/*.eml` so the seed binary has no runtime fixture
/// path lookup.
const FIXTURES: &[(&str, &[u8])] = &[
    (
        "simple_text",
        include_bytes!("../../tests/fixtures/simple_text.eml"),
    ),
    (
        "multipart",
        include_bytes!("../../tests/fixtures/multipart.eml"),
    ),
    (
        "thread_chain",
        include_bytes!("../../tests/fixtures/thread_chain.eml"),
    ),
    (
        "attachment_multipart",
        include_bytes!("../../tests/fixtures/attachment_multipart.eml"),
    ),
    (
        "nested_quotes",
        include_bytes!("../../tests/fixtures/nested_quotes.eml"),
    ),
    (
        "non_ascii",
        include_bytes!("../../tests/fixtures/non_ascii.eml"),
    ),
    (
        "no_message_id",
        include_bytes!("../../tests/fixtures/no_message_id.eml"),
    ),
    (
        "malformed",
        include_bytes!("../../tests/fixtures/malformed.eml"),
    ),
];

/// Demo MCP gate seeds — one row per [`postblox::models::GateAction`]
/// variant so every approval-flow code path has data to render.
const DEMO_GATES: &[(&str, &str, &str, &str)] = &[
    (
        "postblox_message_send",
        r#"{"to":"*@demo.local"}"#,
        "auto_allow",
        "demo: auto-allow internal sends",
    ),
    (
        "postblox_message_delete",
        r#"{"folder":"Trash"}"#,
        "deny",
        "demo: never auto-delete from Trash",
    ),
    (
        "postblox_draft_create",
        r#"{}"#,
        "require",
        "demo: require approval for new drafts",
    ),
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let socket_path: PathBuf = std::env::var_os("POSTBLOX_SOCKET")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("POSTBLOX_SOCKET is required"))?;
    let db_path: PathBuf = std::env::var_os("POSTBLOX_DB")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("POSTBLOX_DB is required"))?;

    eprintln!(
        "seed: socket={} db={}",
        socket_path.display(),
        db_path.display()
    );

    let mut client = Client::connect(&socket_path)
        .await
        .with_context(|| format!("connect to {}", socket_path.display()))?;
    let pool = db::connect(&db_path)
        .await
        .with_context(|| format!("open db at {}", db_path.display()))?;

    let accounts = seed_accounts(&mut client).await?;
    println!("seed: {} accounts ok", accounts.len());

    let folders = seed_folders(&mut client, &accounts).await?;
    println!("seed: {} folders ok", folders.len());

    let messages_inserted = seed_messages(&pool, &accounts, &folders).await?;
    println!("seed: {messages_inserted} messages ok");

    let drafts_inserted = seed_drafts(&mut client, &accounts).await?;
    println!("seed: {drafts_inserted} drafts ok");

    let gates_inserted = seed_gates(&mut client).await?;
    println!("seed: {gates_inserted} gate rules ok");

    let approvals_inserted = seed_approvals(&mut client).await?;
    println!("seed: {approvals_inserted} pending approvals ok");

    Ok(())
}

/// Account row materialised after either lookup or fresh creation.
struct AccountRecord {
    id: AccountId,
    email: &'static str,
    topics: &'static [&'static str],
}

async fn seed_accounts(client: &mut Client) -> anyhow::Result<Vec<AccountRecord>> {
    let existing = client
        .request("account.list", json!({}))
        .await
        .context("account.list")?;
    require_ok(&existing, "account.list")?;
    let existing_arr = existing.data.as_array().cloned().unwrap_or_default();

    let mut out = Vec::with_capacity(DEMO_ACCOUNTS.len());
    for acc in DEMO_ACCOUNTS {
        if let Some(found) = existing_arr.iter().find(|row| row["email"] == acc.email) {
            let id = parse_account_id(found, "id")?;
            out.push(AccountRecord {
                id,
                email: acc.email,
                topics: acc.topics,
            });
            continue;
        }
        let resp = client
            .request(
                "account.create",
                json!({
                    "email": acc.email,
                    "display_name": acc.display_name,
                    "auth_kind": "password",
                    "imap_host": "imap.demo.local",
                    "imap_port": 993,
                    "imap_use_tls": true,
                    "smtp_host": "smtp.demo.local",
                    "smtp_port": 465,
                    "smtp_use_tls": true,
                    "smtp_starttls": false,
                }),
            )
            .await
            .with_context(|| format!("account.create {}", acc.email))?;
        require_ok(&resp, "account.create")?;
        let id = parse_account_id(&resp.data, "id")?;
        out.push(AccountRecord {
            id,
            email: acc.email,
            topics: acc.topics,
        });
    }
    Ok(out)
}

/// Folder rows materialised by name, keyed by `(account_id, name)`.
struct FolderRecord {
    account_id: AccountId,
    role: FolderRole,
    id: FolderId,
}

async fn seed_folders(
    client: &mut Client,
    accounts: &[AccountRecord],
) -> anyhow::Result<Vec<FolderRecord>> {
    let mut out = Vec::with_capacity(accounts.len() * DEMO_FOLDERS.len());
    for acc in accounts {
        for (name, role) in DEMO_FOLDERS {
            let resp = client
                .request(
                    "folder.upsert",
                    json!({
                        "account_id": acc.id.to_string(),
                        "name": name,
                        "delimiter": "/",
                        "role": role.as_str(),
                        "selectable": true,
                    }),
                )
                .await
                .with_context(|| format!("folder.upsert {} for {}", name, acc.email))?;
            require_ok(&resp, "folder.upsert")?;
            let id = parse_folder_id(&resp.data, "id")?;
            out.push(FolderRecord {
                account_id: acc.id,
                role: *role,
                id,
            });
        }
    }
    Ok(out)
}

/// Insert message rows directly into SQLite. Returns the number of rows
/// inserted on this run (skipped rows are not counted).
async fn seed_messages(
    pool: &SqlitePool,
    accounts: &[AccountRecord],
    folders: &[FolderRecord],
) -> anyhow::Result<usize> {
    let now = Utc::now();
    let mut inserted = 0;
    for (account_idx, acc) in accounts.iter().enumerate() {
        let inbox = require_folder(folders, acc.id, FolderRole::Inbox)?;
        let sent = require_folder(folders, acc.id, FolderRole::Sent)?;
        let archive = require_folder(folders, acc.id, FolderRole::Archive)?;

        // INBOX: per topic, seed a thread of `INBOX_MESSAGES_PER_THREAD` messages.
        for (topic_idx, topic) in acc.topics.iter().enumerate() {
            inserted += seed_thread(
                pool,
                acc,
                inbox.id,
                topic,
                topic_idx,
                INBOX_MESSAGES_PER_THREAD,
                "inbox",
                account_idx,
                now,
            )
            .await?;
        }
        // Sent: short standalone messages, each its own thread.
        for sent_idx in 0..SENT_MESSAGES_PER_ACCOUNT {
            inserted += seed_thread(
                pool,
                acc,
                sent.id,
                &format!("Sent reply #{sent_idx}"),
                sent_idx,
                1,
                "sent",
                account_idx,
                now,
            )
            .await?;
        }
        // Archive: short standalone messages, each its own thread.
        for archive_idx in 0..ARCHIVE_MESSAGES_PER_ACCOUNT {
            inserted += seed_thread(
                pool,
                acc,
                archive.id,
                &format!("Old archived note #{archive_idx}"),
                archive_idx,
                1,
                "archive",
                account_idx,
                now,
            )
            .await?;
        }
    }
    Ok(inserted)
}

/// Seed a single thread of `count` chained messages. Idempotent on the
/// thread's `external_id`; if the thread already exists, every message
/// in it is left alone.
#[allow(clippy::too_many_arguments)]
async fn seed_thread(
    pool: &SqlitePool,
    acc: &AccountRecord,
    folder_id: FolderId,
    subject: &str,
    topic_idx: usize,
    count: usize,
    folder_tag: &str,
    account_idx: usize,
    base_time: DateTime<Utc>,
) -> anyhow::Result<usize> {
    let external_id = format!("demo-{}-{folder_tag}-{topic_idx}", acc.email);
    if db::threads::get_by_external_id(pool, acc.id, &external_id)
        .await
        .with_context(|| format!("threads::get_by_external_id {external_id}"))?
        .is_some()
    {
        return Ok(0);
    }

    let thread = db::threads::create(pool, acc.id, Some(&external_id), Some(subject))
        .await
        .with_context(|| format!("threads::create {external_id}"))?;

    let mut inserted = 0;
    for msg_idx in 0..count {
        inserted += seed_one_message(
            pool,
            acc,
            folder_id,
            thread.id,
            subject,
            topic_idx,
            msg_idx,
            folder_tag,
            account_idx,
            base_time,
        )
        .await?;
    }
    db::threads::refresh_aggregates(pool, thread.id)
        .await
        .with_context(|| format!("threads::refresh_aggregates {}", thread.id))?;
    Ok(inserted)
}

/// Build one message row from a fixture, with header rewrites that make
/// it look distinct per account / topic / sequence number.
#[allow(clippy::too_many_arguments)]
async fn seed_one_message(
    pool: &SqlitePool,
    acc: &AccountRecord,
    folder_id: FolderId,
    thread_id: ThreadId,
    subject: &str,
    topic_idx: usize,
    msg_idx: usize,
    folder_tag: &str,
    account_idx: usize,
    base_time: DateTime<Utc>,
) -> anyhow::Result<usize> {
    // Deterministic UID derived from the same key tuple used by
    // `external_id`: stable across reseeds, unique within a folder.
    let uid = encode_uid(account_idx, topic_idx, msg_idx, folder_tag);
    if db::messages::get_by_folder_uid(pool, folder_id, uid)
        .await
        .with_context(|| format!("messages::get_by_folder_uid {folder_id} {uid}"))?
        .is_some()
    {
        return Ok(0);
    }

    let fixture_idx = (account_idx + topic_idx + msg_idx) % FIXTURES.len();
    let (fixture_name, raw_bytes) = FIXTURES[fixture_idx];
    let parsed = postblox::mail::parser::parse(raw_bytes).with_context(|| {
        format!(
            "parse fixture {} for {} thread {topic_idx} msg {msg_idx}",
            fixture_name, acc.email
        )
    })?;

    let from_addr = match folder_tag {
        "sent" | "archive" => acc.email.to_string(),
        _ => format!("contact-{account_idx}-{topic_idx}@demo.example"),
    };
    let to_addrs: AddressList = match folder_tag {
        "sent" => AddressList::from(vec![format!(
            "contact-{account_idx}-{topic_idx}@demo.example"
        )]),
        _ => AddressList::from(vec![acc.email.to_string()]),
    };
    let display_subject = if msg_idx == 0 {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    };
    let snippet = parsed
        .text_body
        .as_deref()
        .map(|body| body.chars().take(140).collect::<String>())
        .unwrap_or_else(|| format!("Demo message body for {subject}"));
    let text_body = parsed.text_body.clone().unwrap_or_else(|| {
        format!(
            "Demo body — {fixture_name} fixture for {} thread {topic_idx}.",
            acc.email
        )
    });
    // Stagger timestamps so list views show realistic ordering.
    let internal_date = base_time
        - Duration::minutes((account_idx as i64) * 17 + (topic_idx as i64) * 5)
        - Duration::seconds((msg_idx as i64) * 30);

    let message_id_header = format!(
        "<demo-{}-{folder_tag}-{topic_idx}-{msg_idx}@demo.local>",
        acc.email
    );
    let in_reply_to = if msg_idx == 0 {
        None
    } else {
        Some(format!(
            "<demo-{}-{folder_tag}-{topic_idx}-{}@demo.local>",
            acc.email,
            msg_idx - 1
        ))
    };
    let references_header = if msg_idx == 0 {
        None
    } else {
        let chain: Vec<String> = (0..msg_idx)
            .map(|i| {
                format!(
                    "<demo-{}-{folder_tag}-{topic_idx}-{i}@demo.local>",
                    acc.email
                )
            })
            .collect();
        Some(chain.join(" "))
    };
    let flags = if matches!(folder_tag, "sent" | "archive") || msg_idx > 0 {
        MessageFlags::from(vec!["\\Seen"])
    } else {
        MessageFlags::default()
    };

    let new_msg = db::messages::NewMessage {
        account_id: acc.id,
        folder_id,
        thread_id: Some(thread_id),
        uid,
        message_id_header: Some(message_id_header),
        in_reply_to,
        references_header,
        from_addr,
        to_addrs,
        cc_addrs: AddressList::default(),
        bcc_addrs: AddressList::default(),
        reply_to: None,
        subject: Some(display_subject),
        snippet: Some(snippet),
        text_body: Some(text_body),
        html_body: parsed.html_body.clone(),
        raw_size: raw_bytes.len() as i64,
        flags,
        internal_date,
        sent_at: Some(internal_date),
    };
    db::messages::create(pool, &new_msg)
        .await
        .with_context(|| format!("messages::create uid={uid} for {}", acc.email))?;
    Ok(1)
}

/// Encode a stable UID per `(account, topic, msg_idx, folder_tag)`. The
/// folder-tag offset prevents collisions between INBOX/Sent/Archive
/// thread chains within the same account.
fn encode_uid(account_idx: usize, topic_idx: usize, msg_idx: usize, folder_tag: &str) -> i64 {
    let folder_offset: i64 = match folder_tag {
        "inbox" => 1_000,
        "sent" => 2_000,
        "archive" => 3_000,
        _ => 9_000,
    };
    folder_offset + (account_idx as i64) * 200 + (topic_idx as i64) * 10 + (msg_idx as i64)
}

async fn seed_drafts(client: &mut Client, accounts: &[AccountRecord]) -> anyhow::Result<usize> {
    // Two drafts: one for the first account, one for the second. Idempotent
    // on `(account_id, subject)` via draft.list.
    let drafts = [
        (
            0usize,
            "Draft: weekly update",
            "Hi team,\n\nQuick weekly summary draft to flesh out before sending.",
        ),
        (
            1usize,
            "Draft: hiring pipeline notes",
            "Notes from the hiring sync — to be circulated tomorrow.",
        ),
    ];
    let mut inserted = 0;
    for (acc_idx, subject, body) in drafts {
        let acc = accounts
            .get(acc_idx)
            .ok_or_else(|| anyhow!("demo account index {acc_idx} out of range"))?;
        let listed = client
            .request("draft.list", json!({"account_id": acc.id.to_string()}))
            .await
            .with_context(|| format!("draft.list for {}", acc.email))?;
        require_ok(&listed, "draft.list")?;
        let already = listed
            .data
            .as_array()
            .map(|rows| rows.iter().any(|row| row["subject"] == subject))
            .unwrap_or(false);
        if already {
            continue;
        }
        let resp = client
            .request(
                "draft.create",
                json!({
                    "account_id": acc.id.to_string(),
                    "to_addrs": [format!("partner-{acc_idx}@demo.example")],
                    "cc_addrs": [],
                    "bcc_addrs": [],
                    "subject": subject,
                    "text_body": body,
                    "html_body": null,
                    "in_reply_to_msg": null,
                    "_actor": "demo-seed",
                }),
            )
            .await
            .with_context(|| format!("draft.create {subject}"))?;
        require_ok(&resp, "draft.create")?;
        inserted += 1;
    }
    Ok(inserted)
}

async fn seed_gates(client: &mut Client) -> anyhow::Result<usize> {
    let listed = client
        .request("mcp.gate.list", json!({}))
        .await
        .context("mcp.gate.list")?;
    require_ok(&listed, "mcp.gate.list")?;
    let existing = listed.data.as_array().cloned().unwrap_or_default();
    let mut inserted = 0;
    for (tool, arg_pattern, action, note) in DEMO_GATES {
        let already = existing.iter().any(|row| {
            row["tool"] == *tool
                && row.get("arg_pattern").and_then(Value::as_str) == Some(*arg_pattern)
        });
        if already {
            continue;
        }
        let resp = client
            .request(
                "mcp.gate.create",
                json!({
                    "tool": tool,
                    "arg_pattern": arg_pattern,
                    "action": action,
                    "note": note,
                    "_actor": "demo-seed",
                }),
            )
            .await
            .with_context(|| format!("mcp.gate.create {tool}"))?;
        require_ok(&resp, "mcp.gate.create")?;
        inserted += 1;
    }
    Ok(inserted)
}

async fn seed_approvals(client: &mut Client) -> anyhow::Result<usize> {
    // Defined inline rather than in a `const` so the `Value` payloads
    // can be expressed via `json!` macros (which are not const).
    let approvals: &[(&str, Value, &str)] = &[
        (
            "postblox_message_send",
            json!({"account_id": "00000000-0000-0000-0000-000000000001", "draft_id": "00000000-0000-0000-0000-0000000000aa"}),
            "demo: send weekly update draft",
        ),
        (
            "postblox_message_delete",
            json!({"message_id": "00000000-0000-0000-0000-0000000000bb"}),
            "demo: delete promotional thread",
        ),
    ];
    let listed = client
        .request("mcp.approval.list", json!({"state": "pending"}))
        .await
        .context("mcp.approval.list")?;
    require_ok(&listed, "mcp.approval.list")?;
    let existing = listed.data.as_array().cloned().unwrap_or_default();
    let mut inserted = 0;
    for (tool, args, summary) in approvals {
        let already = existing.iter().any(|row| row["summary"] == *summary);
        if already {
            continue;
        }
        let resp = client
            .request(
                "mcp.approval.create",
                json!({
                    "tool": tool,
                    "args": args,
                    "summary": summary,
                    "_actor": "demo-seed",
                }),
            )
            .await
            .with_context(|| format!("mcp.approval.create {summary}"))?;
        require_ok(&resp, "mcp.approval.create")?;
        inserted += 1;
    }
    Ok(inserted)
}

fn require_folder(
    folders: &[FolderRecord],
    account_id: AccountId,
    role: FolderRole,
) -> anyhow::Result<&FolderRecord> {
    folders
        .iter()
        .find(|f| f.account_id == account_id && f.role == role)
        .ok_or_else(|| anyhow!("folder with role {} missing for account", role.as_str()))
}

fn require_ok(resp: &postblox::ipc::Response, op: &str) -> anyhow::Result<()> {
    if resp.ok {
        Ok(())
    } else {
        let err = resp.error.clone();
        Err(anyhow!(
            "ipc op {op} failed: {}",
            err.map(|e| format!("{}: {}", e.code, e.message))
                .unwrap_or_else(|| "unknown error".into())
        ))
    }
}

fn parse_account_id(value: &Value, field: &str) -> anyhow::Result<AccountId> {
    let s = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing {field} on {value}"))?;
    AccountId::from_str(s).with_context(|| format!("parse {field} as AccountId"))
}

fn parse_folder_id(value: &Value, field: &str) -> anyhow::Result<FolderId> {
    let s = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing {field} on {value}"))?;
    FolderId::from_str(s).with_context(|| format!("parse {field} as FolderId"))
}
