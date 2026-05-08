#!/usr/bin/env bash
# H1 perf baseline harness.
#
# Builds release, starts a fresh `postbloxd` against a temp DB + temp socket,
# runs each measurement, and prints the results in a stable, parseable form.
# Companion file `bench/baseline.md` is filled in by hand from these numbers.
#
# Reproduce: `bash bench/run.sh`
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# --- isolation ---
WORK="$(mktemp -d -t postblox-bench-XXXXXX)"
trap 'cleanup' EXIT
cleanup() {
    if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}

SOCK="$WORK/postblox.sock"
DB="$WORK/postblox.db"
CONFIG="$WORK/postblox.toml"
LOG="$WORK/daemon.log"

# Use the file secrets backend so the keyring isn't touched in CI/headless envs.
cat >"$CONFIG" <<EOF
[secrets]
backend = "file"
passphrase = "bench-only-passphrase"
path = "$WORK/secrets.bin"
EOF

export POSTBLOX_SOCKET="$SOCK"
export POSTBLOX_DB="$DB"
export POSTBLOX_CONFIG="$CONFIG"
export RUST_LOG="${RUST_LOG:-warn}"

echo "==> System info"
uname -a
echo
echo "==> CPU"
( command -v lscpu >/dev/null && lscpu | grep -E 'Model name|Architecture|CPU\(s\)|MHz' ) || \
    grep -E '^(model name|cpu MHz|cpu cores)' /proc/cpuinfo | head -n 4
echo
echo "==> Memory"
grep -E '^(MemTotal|MemAvailable):' /proc/meminfo
echo
echo "==> Toolchain"
rustc --version
echo

echo "==> Build (release)"
cargo build --release --quiet --bins
echo

DAEMON_BIN="$REPO_ROOT/target/release/postbloxd"
BENCH_BIN="$REPO_ROOT/target/release/postblox-bench"
TUI_BIN="$REPO_ROOT/target/release/postblox"

echo "==> Binary sizes (stripped per release profile)"
for b in "$DAEMON_BIN" "$BENCH_BIN" "$TUI_BIN"; do
    if [[ -x "$b" ]]; then
        size=$(stat -c '%s' "$b")
        printf '  %-40s %10d bytes (%.2f MiB)\n' "$(basename "$b")" "$size" "$(awk "BEGIN{print $size/1048576}")"
    fi
done
echo

# --- Cold start: median of 10 runs measuring time from invocation to socket ready ---
echo "==> Cold start (10 runs)"
COLDSTART_MS=()
for i in $(seq 1 10); do
    rm -f "$SOCK"
    t0_ns=$(date +%s%N)
    "$DAEMON_BIN" >"$LOG.$i" 2>&1 &
    pid=$!
    # Poll for socket existence; cap at 5s so a hung daemon doesn't wedge.
    for _ in $(seq 1 5000); do
        if [[ -S "$SOCK" ]]; then
            break
        fi
        # ~1ms granularity
        sleep 0.001
    done
    t1_ns=$(date +%s%N)
    dt_ms=$(( (t1_ns - t0_ns) / 1000000 ))
    COLDSTART_MS+=("$dt_ms")
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    rm -f "$SOCK"
done
printf '  raw_ms: %s\n' "${COLDSTART_MS[*]}"
COLD_SORTED=$(printf '%s\n' "${COLDSTART_MS[@]}" | sort -n | tr '\n' ' ')
COLD_MEDIAN=$(printf '%s\n' "${COLDSTART_MS[@]}" | sort -n | awk 'NR==5')
printf '  sorted: %s\n' "$COLD_SORTED"
printf '  median_ms=%s\n' "$COLD_MEDIAN"
echo

# --- Boot the daemon for the rest of the measurements ---
echo "==> Start daemon"
"$DAEMON_BIN" >"$LOG" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 5000); do
    if [[ -S "$SOCK" ]]; then break; fi
    sleep 0.001
done
if [[ ! -S "$SOCK" ]]; then
    echo "daemon failed to bind socket" >&2
    cat "$LOG" >&2
    exit 1
fi
sleep 2  # let the daemon settle before sampling memory

# --- Idle memory: 5 samples of VmRSS, 1s apart, report median ---
echo "==> Idle memory (5 samples, 1s apart)"
RSS_KB=()
for _ in $(seq 1 5); do
    rss=$(awk '/^VmRSS:/ {print $2}' "/proc/$DAEMON_PID/status")
    RSS_KB+=("$rss")
    sleep 1
done
printf '  raw_kb: %s\n' "${RSS_KB[*]}"
RSS_MEDIAN_KB=$(printf '%s\n' "${RSS_KB[@]}" | sort -n | awk 'NR==3')
RSS_MEDIAN_MB=$(awk "BEGIN{printf \"%.2f\", $RSS_MEDIAN_KB/1024}")
printf '  median_kb=%s (%s MB)\n' "$RSS_MEDIAN_KB" "$RSS_MEDIAN_MB"
echo

# --- IPC ping p50 ---
echo "==> IPC ping p50 (n=1000)"
PING_OUT=$("$BENCH_BIN" ping 1000)
echo "  raw: $PING_OUT  (min,p50,p95,max us)"
echo

# --- DB read p50 ---
echo "==> DB read p50 — account.list (n=1000)"
DB_OUT=$("$BENCH_BIN" db-read 1000)
echo "  raw: $DB_OUT  (min,p50,p95,max us)"
echo

# --- Email parsing throughput ---
echo "==> Email parsing throughput (n=10000, corpus=100)"
PARSE_OUT=$("$BENCH_BIN" parse 10000)
echo "  raw: $PARSE_OUT  (messages,elapsed_ms,msgs_per_sec)"
echo

echo "==> Done. Update bench/baseline.md from the numbers above."
