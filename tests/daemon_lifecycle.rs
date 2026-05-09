//! Integration tests for `postbloxd`'s graceful shutdown handling.
//!
//! Spawns the real daemon binary as a subprocess, confirms it is
//! listening on its socket, signals it, and asserts a clean exit plus
//! socket-file cleanup. Guards against regressions where the daemon
//! stops handling SIGTERM (or someone "simplifies" the helper back to
//! SIGINT-only) and against the socket file being left on disk after a
//! graceful exit.
//!
//! Unix-only: SIGTERM doesn't exist on Windows.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use postblox::ipc::client::Client;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Instant};

/// Hard cap on how long a single test waits for any single step. Keeps
/// the suite well under `cargo test`'s default 60s without being so
/// tight that a slow CI runner flakes.
const STEP_TIMEOUT: Duration = Duration::from_secs(2);

struct Daemon {
    child: Child,
    sock: PathBuf,
    _tmp: TempDir,
}

impl Daemon {
    /// Spawn `postbloxd` against a fresh temp dir + socket and wait
    /// until it is actually accepting connections.
    async fn spawn() -> Self {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let sock = tmp.path().join("postbloxd.sock");
        let db_path = tmp.path().join("postblox.db");
        let config_path = tmp.path().join("postblox.toml");
        // File-backed secrets store keeps the daemon out of the user's
        // OS keyring on shared CI runners (and avoids any DBus / Secret
        // Service prompts in headless environments).
        std::fs::write(
            &config_path,
            "[secrets]\nbackend = \"file\"\npassphrase = \"daemon-lifecycle-test\"\n",
        )
        .expect("write test config");

        let bin = env!("CARGO_BIN_EXE_postbloxd");
        let child = Command::new(bin)
            .env("POSTBLOX_SOCKET", &sock)
            .env("POSTBLOX_DB", &db_path)
            .env("POSTBLOX_CONFIG", &config_path)
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
            _tmp: tmp,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Wait for the child to exit, with a hard cap. Returns the exit
    /// status if it exited in time, or `None` if the wait timed out
    /// (caller is then responsible for forcefully killing it).
    fn wait_for_exit(&mut self, max: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + max;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Best-effort: if a test panicked before signaling we still want
        // to reap the child so subsequent tests don't fight over PIDs.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Poll for the socket file then connect + `ping` to confirm the
/// daemon's accept loop is actually running. Bounded by `STEP_TIMEOUT`.
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

/// Send a signal to `pid` by shelling out to `kill(1)`. Avoids pulling
/// in `nix` or `libc` as direct deps (anti-bloat).
fn send_signal(pid: u32, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .expect("invoke kill");
    assert!(status.success(), "kill {signal} {pid} failed: {status}");
}

#[tokio::test]
async fn test_daemon_clean_shutdown_on_sigterm() {
    let mut daemon = Daemon::spawn().await;
    let sock = daemon.sock.clone();

    send_signal(daemon.pid(), "-TERM");

    let status = daemon
        .wait_for_exit(STEP_TIMEOUT)
        .expect("daemon did not exit within timeout after SIGTERM");
    assert!(
        status.success(),
        "daemon exited non-zero after SIGTERM: {status:?}"
    );
    assert!(
        !sock.exists(),
        "socket file {sock:?} should be removed after graceful shutdown"
    );
}

#[tokio::test]
async fn test_daemon_clean_shutdown_on_sigint() {
    let mut daemon = Daemon::spawn().await;
    let sock = daemon.sock.clone();

    send_signal(daemon.pid(), "-INT");

    let status = daemon
        .wait_for_exit(STEP_TIMEOUT)
        .expect("daemon did not exit within timeout after SIGINT");
    assert!(
        status.success(),
        "daemon exited non-zero after SIGINT: {status:?}"
    );
    assert!(
        !sock.exists(),
        "socket file {sock:?} should be removed after graceful shutdown"
    );
}

#[tokio::test]
async fn test_daemon_socket_left_when_killed_with_sigkill() {
    // Negative control: SIGKILL bypasses the graceful path, so the
    // socket file must still be present afterwards. This pins the
    // "graceful vs forceful" contract — if the daemon ever started
    // installing some kind of pre-exec hook, this test would fail.
    let mut daemon = Daemon::spawn().await;
    let sock = daemon.sock.clone();

    send_signal(daemon.pid(), "-KILL");

    let status = daemon
        .wait_for_exit(STEP_TIMEOUT)
        .expect("daemon did not exit within timeout after SIGKILL");
    assert!(
        !status.success(),
        "process killed by SIGKILL should not report success: {status:?}"
    );
    assert!(
        sock.exists(),
        "socket file {sock:?} should remain after SIGKILL (no graceful cleanup)"
    );
}
