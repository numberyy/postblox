//! End-to-end tests for the `postblox-demo-seed` binary.
//!
//! Spawns the real `postbloxd` against a tempdir, runs the seed binary
//! against the same socket / DB, and asserts the expected entity counts.
//! Re-runs the seed to confirm idempotency (counts unchanged).
//!
//! Unix-only: matches `tests/daemon_lifecycle.rs` for the same reason
//! (the seed binary connects over a Unix socket).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use postblox::ipc::client::Client;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

/// Same shape and intent as `tests/daemon_lifecycle.rs`'s `STEP_TIMEOUT`.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

struct Daemon {
    child: Child,
    sock: PathBuf,
    db: PathBuf,
    _tmp: TempDir,
}

impl Daemon {
    async fn spawn() -> Self {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let sock = tmp.path().join("postbloxd.sock");
        let db = tmp.path().join("postblox.db");
        let config = tmp.path().join("postblox.toml");
        std::fs::write(
            &config,
            "[secrets]\nbackend = \"file\"\npassphrase = \"demo-seed-test\"\n",
        )
        .expect("write test config");

        let bin = env!("CARGO_BIN_EXE_postbloxd");
        let child = Command::new(bin)
            .env("POSTBLOX_SOCKET", &sock)
            .env("POSTBLOX_DB", &db)
            .env("POSTBLOX_CONFIG", &config)
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn postbloxd");

        wait_until_listening(&sock).await;

        Self {
            child,
            sock,
            db,
            _tmp: tmp,
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

async fn wait_until_listening(sock: &Path) {
    timeout(STEP_TIMEOUT, async {
        loop {
            if sock.exists() {
                if let Ok(mut client) = Client::connect(sock).await {
                    if let Ok(resp) = client.request("ping", json!({})).await {
                        if resp.ok {
                            return;
                        }
                    }
                }
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("daemon never started listening");
}

fn run_seed(daemon: &Daemon) {
    let bin = env!("CARGO_BIN_EXE_postblox-demo-seed");
    let output = Command::new(bin)
        .env("POSTBLOX_SOCKET", &daemon.sock)
        .env("POSTBLOX_DB", &daemon.db)
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn postblox-demo-seed");
    assert!(
        output.status.success(),
        "seed failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[derive(Debug, Clone, Copy)]
struct SeededCounts {
    accounts: usize,
    folders: usize,
    messages: usize,
    drafts: usize,
    gates: usize,
    pending_approvals: usize,
    attachments: usize,
    messages_with_attachments: usize,
}

async fn count_state(daemon: &Daemon) -> SeededCounts {
    let mut client = Client::connect(&daemon.sock).await.expect("connect");

    let accounts_resp = client
        .request("account.list", json!({}))
        .await
        .expect("account.list");
    assert!(accounts_resp.ok, "account.list: {:?}", accounts_resp.error);
    let accounts = accounts_resp.data.as_array().expect("array").clone();

    let mut folders_total = 0;
    let mut messages_total = 0;
    let mut drafts_total = 0;
    let mut attachments_total = 0;
    let mut messages_with_attachments = 0;
    for acc in &accounts {
        let acc_id = acc["id"].as_str().expect("account id is string");
        let folders_resp = client
            .request("folder.list", json!({"account_id": acc_id}))
            .await
            .expect("folder.list");
        assert!(folders_resp.ok, "folder.list: {:?}", folders_resp.error);
        let folders = folders_resp.data.as_array().expect("array").clone();
        folders_total += folders.len();
        for folder in &folders {
            let folder_id = folder["id"].as_str().expect("folder id is string");
            let msgs_resp = client
                .request(
                    "message.list_by_folder",
                    json!({"folder_id": folder_id, "limit": 500}),
                )
                .await
                .expect("message.list_by_folder");
            assert!(
                msgs_resp.ok,
                "message.list_by_folder: {:?}",
                msgs_resp.error
            );
            let msgs = msgs_resp.data.as_array().cloned().unwrap_or_default();
            messages_total += msgs.len();
            for msg in &msgs {
                let msg_id = msg["id"].as_str().expect("message id is string");
                let att_resp = client
                    .request("attachment.list", json!({"message_id": msg_id}))
                    .await
                    .expect("attachment.list");
                assert!(att_resp.ok, "attachment.list: {:?}", att_resp.error);
                let count = att_resp.data.as_array().map(Vec::len).unwrap_or(0);
                if count > 0 {
                    messages_with_attachments += 1;
                }
                attachments_total += count;
            }
        }

        let drafts_resp = client
            .request("draft.list", json!({"account_id": acc_id}))
            .await
            .expect("draft.list");
        assert!(drafts_resp.ok, "draft.list: {:?}", drafts_resp.error);
        drafts_total += drafts_resp.data.as_array().map(Vec::len).unwrap_or(0);
    }

    let gates_resp = client
        .request("mcp.gate.list", json!({}))
        .await
        .expect("mcp.gate.list");
    assert!(gates_resp.ok, "mcp.gate.list: {:?}", gates_resp.error);
    let gates = gates_resp.data.as_array().map(Vec::len).unwrap_or(0);

    let approvals_resp = client
        .request("mcp.approval.list", json!({"state": "pending"}))
        .await
        .expect("mcp.approval.list");
    assert!(
        approvals_resp.ok,
        "mcp.approval.list: {:?}",
        approvals_resp.error
    );
    let pending = approvals_resp.data.as_array().map(Vec::len).unwrap_or(0);

    SeededCounts {
        accounts: accounts.len(),
        folders: folders_total,
        messages: messages_total,
        drafts: drafts_total,
        gates,
        pending_approvals: pending,
        attachments: attachments_total,
        messages_with_attachments,
    }
}

async fn assert_pending_approval_targets_resolve(daemon: &Daemon) {
    let mut client = Client::connect(&daemon.sock).await.expect("connect");
    let approvals_resp = client
        .request("mcp.approval.list", json!({"state": "pending"}))
        .await
        .expect("mcp.approval.list");
    assert!(
        approvals_resp.ok,
        "mcp.approval.list: {:?}",
        approvals_resp.error
    );
    let approvals = approvals_resp.data.as_array().expect("approval array");
    let mut saw_message = false;
    let mut saw_draft = false;

    for approval in approvals {
        let args = approval
            .get("args")
            .and_then(serde_json::Value::as_object)
            .expect("approval args object");
        if let Some(message_id) = args.get("message_id").and_then(serde_json::Value::as_str) {
            let message_resp = client
                .request("message.get", json!({"id": message_id}))
                .await
                .expect("message.get");
            assert!(message_resp.ok, "message.get: {:?}", message_resp.error);
            assert!(
                message_resp
                    .data
                    .get("subject")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|subject| subject.contains("Quarterly review draft")),
                "approval message should resolve to the seeded quarterly review message"
            );
            assert!(
                message_resp
                    .data
                    .get("from_addr")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|from| from.contains("@demo.example")),
                "approval message should expose a meaningful sender"
            );
            saw_message = true;
        }
        if let Some(draft_id) = args.get("draft_id").and_then(serde_json::Value::as_str) {
            let draft_resp = client
                .request("draft.get", json!({"id": draft_id}))
                .await
                .expect("draft.get");
            assert!(draft_resp.ok, "draft.get: {:?}", draft_resp.error);
            let draft = draft_resp.data.get("draft").expect("draft payload");
            assert_eq!(
                draft.get("subject").and_then(serde_json::Value::as_str),
                Some("Draft: weekly update")
            );
            assert!(
                draft
                    .get("to_addrs")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|to| !to.is_empty()),
                "approval draft should expose recipients"
            );
            saw_draft = true;
        }
    }

    assert!(saw_message, "expected a pending approval with message_id");
    assert!(saw_draft, "expected a pending approval with draft_id");
}

#[tokio::test]
async fn test_demo_seed_populates_minimum_entity_counts() {
    let daemon = Daemon::spawn().await;
    run_seed(&daemon);

    let counts = count_state(&daemon).await;
    assert_pending_approval_targets_resolve(&daemon).await;
    assert!(
        counts.accounts >= 3,
        "expected >=3 accounts, got {}",
        counts.accounts
    );
    assert!(
        counts.folders >= 12,
        "expected >=12 folders, got {}",
        counts.folders
    );
    assert!(
        counts.messages >= 90,
        "expected >=90 messages, got {}",
        counts.messages
    );
    assert!(
        counts.drafts >= 2,
        "expected >=2 drafts, got {}",
        counts.drafts
    );
    assert!(
        counts.gates >= 3,
        "expected >=3 gate rules, got {}",
        counts.gates
    );
    assert!(
        counts.pending_approvals >= 2,
        "expected >=2 pending approvals, got {}",
        counts.pending_approvals
    );
    // At least one seeded message has to carry an attachment row so the
    // TUI's attachment pane has something to render in demo mode.
    assert!(
        counts.messages_with_attachments >= 1,
        "expected >=1 message with attachments, got {}",
        counts.messages_with_attachments
    );
    // The seed picks fixtures via `(account_idx + topic_idx + msg_idx) %
    // FIXTURES.len()` where only `attachment_multipart.eml` (index 3 in
    // an 8-slot table) carries an attachment part — `multipart.eml`
    // and the other fixtures parse to zero attachments. Walking the
    // INBOX/Sent/Archive grid for all 3 demo accounts at the
    // documented `INBOX_MESSAGES_PER_THREAD = 6`, `SENT = 3`, `ARCHIVE
    // = 2` shape yields 13 hits on fixture index 3 (10 INBOX + 2 Sent
    // + 1 Archive). A floor of 3 keeps the assert resilient if the
    // fixture table or per-account thread counts ever shift slightly,
    // while still being well above the "single accidental insert"
    // threshold.
    assert!(
        counts.attachments >= 3,
        "expected >=3 attachment rows seeded, got {}",
        counts.attachments
    );
}

#[tokio::test]
async fn test_demo_seed_is_idempotent() {
    let daemon = Daemon::spawn().await;
    run_seed(&daemon);
    let first = count_state(&daemon).await;
    run_seed(&daemon);
    let second = count_state(&daemon).await;

    assert_eq!(second.accounts, first.accounts);
    assert_eq!(second.folders, first.folders);
    assert_eq!(second.messages, first.messages);
    assert_eq!(second.drafts, first.drafts);
    assert_eq!(second.gates, first.gates);
    assert_eq!(second.pending_approvals, first.pending_approvals);
    assert_eq!(second.attachments, first.attachments);
    assert_eq!(
        second.messages_with_attachments,
        first.messages_with_attachments
    );
}
