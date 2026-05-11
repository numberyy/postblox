default:
    @just --list

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

run *ARGS:
    cargo run -- {{ARGS}}

test:
    cargo test --workspace

fmt:
    cargo fmt --all

build:
    cargo build --workspace --release

deny:
    cargo deny check

mail-deps:
    cargo tree -p postblox-mail --edges normal > /tmp/postblox-mail-tree.txt
    if grep -E '\b(tokio|sqlx|reqwest|ratatui|lettre) v' /tmp/postblox-mail-tree.txt; then cat /tmp/postblox-mail-tree.txt; exit 1; fi

# Spin up a self-contained local demo: builds release, starts a fresh
# `postbloxd` against a tempdir, seeds it with demo accounts/folders/
# messages/drafts/MCP gates/approvals, starts the MCP shim, and exec's
# the TUI. Tears everything down (daemon + shim + tempdir) on exit.
demo:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --workspace --release --quiet
    WORK="$(mktemp -d -t postblox-demo-XXXXXX)"
    SOCK="$WORK/postblox.sock"
    DB="$WORK/postblox.db"
    CONFIG="$WORK/postblox.toml"
    DAEMON_LOG="$WORK/daemon.log"
    MCP_LOG="$WORK/mcp.log"
    SEED_LOG="$WORK/seed.log"
    cat >"$CONFIG" <<EOF
    [secrets]
    backend = "file"
    passphrase = "demo"
    path = "$WORK/secrets.bin"
    EOF
    export POSTBLOX_SOCKET="$SOCK"
    export POSTBLOX_DB="$DB"
    export POSTBLOX_CONFIG="$CONFIG"
    export RUST_LOG="${RUST_LOG:-warn}"
    echo "demo: workdir=$WORK"
    echo "demo: socket=$SOCK"
    echo "demo: db=$DB"
    echo "demo: daemon log=$DAEMON_LOG"
    echo "demo: mcp log=$MCP_LOG"
    echo "demo: seed log=$SEED_LOG"
    "$PWD/target/release/postbloxd" >"$DAEMON_LOG" 2>&1 &
    DAEMON_PID=$!
    MCP_PID=""
    cleanup() {
        if [[ -n "${MCP_PID:-}" ]] && kill -0 "$MCP_PID" 2>/dev/null; then
            kill "$MCP_PID" 2>/dev/null || true
            wait "$MCP_PID" 2>/dev/null || true
        fi
        if kill -0 "$DAEMON_PID" 2>/dev/null; then
            kill "$DAEMON_PID" 2>/dev/null || true
            wait "$DAEMON_PID" 2>/dev/null || true
        fi
        rm -rf "$WORK"
    }
    trap cleanup EXIT
    for _ in $(seq 1 5000); do
        if [[ -S "$SOCK" ]]; then break; fi
        sleep 0.001
    done
    if [[ ! -S "$SOCK" ]]; then
        echo "demo: daemon failed to bind socket" >&2
        cat "$DAEMON_LOG" >&2 || true
        exit 1
    fi
    if ! "$PWD/target/release/postblox-demo-seed" >"$SEED_LOG" 2>&1; then
        echo "demo: seed failed; see $SEED_LOG" >&2
        cat "$SEED_LOG" >&2 || true
        exit 1
    fi
    cat "$SEED_LOG"
    "$PWD/target/release/postblox-mcp" >"$MCP_LOG" 2>&1 &
    MCP_PID=$!
    # Run TUI in the foreground; trap fires on its exit (TTY exit, error,
    # or signal) and shuts down the daemon/shim + removes the tempdir.
    "$PWD/target/release/postblox" || true
