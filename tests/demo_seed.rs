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
            messages_total += msgs_resp.data.as_array().map(Vec::len).unwrap_or(0);
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
    }
}

#[tokio::test]
async fn test_demo_seed_populates_minimum_entity_counts() {
    let daemon = Daemon::spawn().await;
    run_seed(&daemon);

    let counts = count_state(&daemon).await;
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
}
