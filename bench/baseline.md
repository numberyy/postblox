# H1 — postblox performance baseline

First measurement of the project against the hard limits in
`CLAUDE.md` § "Performance Targets". Measurement-only — no optimisation
work was done as part of this slice.

## How to reproduce

```sh
bash bench/run.sh
```

The script:
1. Builds release binaries (`opt-level=z`, `lto=true`, `strip=true`,
   `panic=abort`).
2. Spawns a fresh `postbloxd` against an empty temp DB and a temp socket
   (file secrets backend, so the OS keyring isn't touched).
3. Runs cold-start (10 runs, median), idle memory (5 samples, median),
   IPC ping p50, DB-read p50, and email parsing throughput.

## Environment (this run)

| Field | Value |
|---|---|
| Date | 2026-05-09 |
| HEAD | `4dff6fe` (R6 Slice 9 — attachment preview scroll and copy) |
| Kernel | `Linux 6.19.11-200.fc43.x86_64` |
| CPU | Intel Core Ultra 7 258V (8 cores) |
| Memory | 32 GiB total / 25 GiB available |
| Toolchain | `rustc 1.94.1 (e408947bf 2026-03-25)` |
| Profile | `[profile.release]` opt-level=z, lto=true, codegen-units=1, strip=true, panic=abort |

## Results

Status legend: ✓ within target · ⚠ within hard limit but over target ·
✗ over hard limit · N/A not measured.

| Metric | Measured | Target | Hard limit | Status |
|---|---|---|---|---|
| Daemon idle memory (RSS, median of 5 @ 1s) | **9.68 MB** (9 908 KB) | < 30 MB | 60 MB | ✓ |
| IPC `ping` round-trip p50 (n=1000) | **14 µs** (min 7, p95 27, max 193) | < 1 ms | 5 ms | ✓ |
| Local DB read p50 — `account.list`, empty DB (n=1000) | **37 µs** (min 26, p95 63, max 771) | < 5 ms | 25 ms | ✓ |
| SMTP submission p50 | not measured (no SMTP target) | < 200 ms | 1 s | N/A |
| Email parsing throughput (mail-parser, 100-msg corpus × 100) | **275 378 msgs/sec** | > 5 000 msgs/sec | — | ✓ |
| Cold start to listening (median of 10) | **24 ms** | < 50 ms | 250 ms | ✓ |
| Binary size — `postbloxd` (stripped) | **5.84 MiB** (6 128 408 B) | < 5 MB | 10 MB | ⚠ |
| Binary size — `postblox` TUI (stripped) | 1.58 MiB (1 660 592 B) | < 5 MB | 10 MB | ✓ |
| Binary size — `postblox-mcp` (stripped) | 1.11 MiB (1 162 224 B) | < 5 MB | 10 MB | ✓ |

### Notes on the ⚠ entry

`postbloxd` ships the IMAP IDLE worker, full SMTP submission stack
(`lettre` + `tokio-rustls`), `keyring`, `aes-gcm`, `argon2`, and the
SQLite pool — it is the heaviest binary by far. The result is within
the 10 MB hard limit and headroom remains; the table's 5 MB target is
flagged here for future tracking, not as an action item per H1's
measurement-only scope.

Per CLAUDE.md the "Binary size (stripped)" entry isn't qualified as
"per binary" or "any binary" — recording all three release binaries
makes the next baseline comparison unambiguous. The sizes above use the
size on disk in MiB and bytes; rounding to decimal MB gives 6.13 MB for
`postbloxd`.

### SMTP submission

Skipped because no local SMTP target is configured. The harness wraps a
real `lettre` submitter inside the daemon, so re-running with a working
`SmtpServer` config + a sink (e.g. `mailpit`) is the natural follow-up
when SMTP becomes part of an integration test.

## Reproduction commands (subset)

```sh
# Build
cargo build --release --bins

# Cold start (one run)
rm -f "$RUNTIME/postblox.sock"
t0=$(date +%s%N) && target/release/postbloxd & \
  while [ ! -S "$RUNTIME/postblox.sock" ]; do sleep 0.001; done
t1=$(date +%s%N) ; echo "$(( (t1 - t0) / 1000000 )) ms"

# Idle memory
awk '/^VmRSS:/{print $2 " kB"}' /proc/$(pgrep -f postbloxd)/status

# IPC ping p50 (n=1000)
target/release/postblox-bench ping 1000

# DB read p50 (n=1000)
target/release/postblox-bench db-read 1000

# Email parser throughput (n=10000)
target/release/postblox-bench parse 10000

# Binary sizes
ls -l target/release/postbloxd target/release/postblox target/release/postblox-mcp
```

## Out of scope (deferred per H1)

- Promoting any of these to a CI-enforced check.
- Optimisation work (e.g. trimming the daemon binary).
- SMTP submission p50 (needs an external server).
- Profiling (flamegraphs, async traces).
