---
name: qa-daemon
description: >
  Functional QA tests for postbloxd, the postblox daemon. Builds the
  binary, spawns it under a run-scoped tempdir, and exercises the IPC
  surface (length-prefixed JSON frames over a Unix socket) for read ops,
  write ops, sync ops, and the event hub. Used by the qa orchestrator
  when the diff touches src/bin/postbloxd.rs, src/daemon/**, src/db/**,
  src/ipc/**, src/sync/**, src/imap/**, src/smtp/**, src/secrets/**,
  src/oauth/**, src/mail/**, src/embeddings/**, src/attachments.rs,
  src/auth.rs, src/config.rs, src/models.rs, or migrations/**.
---

# qa-daemon — postbloxd functional QA

## Testing Target

postblox is a local-first single-user app — there are no remote
deployments. **Always test against a fresh local daemon spawned from the
PR branch's compiled binaries**, never against a system-installed daemon
or any remote endpoint.

## Authentication in CI

The daemon's `[secrets]` backend is set to `"file"` (AES-GCM, Argon2id)
for QA runs so the OS keyring is never touched. The passphrase is the
literal dummy string `qa-dummy-pass`. There are no GitHub secrets needed
for daemon QA.

## Pre-flight checks

1. `cargo build --bin postbloxd --locked` — must succeed. Build output
   should appear at `target/debug/postbloxd`.
2. Create a run-scoped tempdir:
   ```sh
   export RUN_ID=$(date -u +%Y%m%dT%H%M%S)-$$
   export RUN_DIR=./qa-results/$RUN_ID
   mkdir -p "$RUN_DIR/data"
   ```
3. Write a minimal `postblox.toml`:
   ```sh
   cat > "$RUN_DIR/postblox.toml" <<'EOF'
   data_dir = "./data"

   [secrets]
   backend = "file"
   passphrase = "qa-dummy-pass"
   EOF
   ```
4. Spawn the daemon with overrides and capture the pid:
   ```sh
   POSTBLOX_DB="$RUN_DIR/postblox.db" \
   POSTBLOX_SOCKET="$RUN_DIR/postbloxd.sock" \
   POSTBLOX_CONFIG="$RUN_DIR/postblox.toml" \
   RUST_LOG=info \
     ./target/debug/postbloxd > "$RUN_DIR/postbloxd.log" 2>&1 &
   echo $! > "$RUN_DIR/postbloxd.pid"
   ```
5. Wait up to 3 seconds for the socket to appear:
   ```sh
   for i in {1..30}; do
     [ -S "$RUN_DIR/postbloxd.sock" ] && break
     sleep 0.1
   done
   [ -S "$RUN_DIR/postbloxd.sock" ] || { echo "BLOCKED: socket never appeared"; exit 1; }
   ```

If any step fails, report all daemon flows as BLOCKED and include the
last 30 lines of `$RUN_DIR/postbloxd.log` as evidence.

## How to talk to the daemon

The IPC protocol is **length-prefixed JSON** over a Unix socket. The
canonical Rust client lives in `tests/ipc_integration.rs` -- read it
once at the start of the run to confirm the wire format hasn't changed.

For QA we use a small inline Python helper (Python 3 is available in CI)
because shell tools don't speak the framing cleanly:

```python
# qa_ipc.py — short helper, inline this in the test step
import json, socket, struct, sys

def call(sock_path, op, args=None, request_id="qa"):
    body = json.dumps({"id": request_id, "op": op, "args": args or {}}).encode()
    frame = struct.pack(">I", len(body)) + body
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(sock_path)
    s.sendall(frame)
    hdr = b""
    while len(hdr) < 4:
        chunk = s.recv(4 - len(hdr))
        if not chunk:
            raise RuntimeError("daemon closed connection")
        hdr += chunk
    n = struct.unpack(">I", hdr)[0]
    body = b""
    while len(body) < n:
        chunk = s.recv(n - len(body))
        if not chunk:
            raise RuntimeError("daemon closed mid-frame")
        body += chunk
    s.close()
    return json.loads(body)

if __name__ == "__main__":
    sock = sys.argv[1]
    op = sys.argv[2]
    args = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    print(json.dumps(call(sock, op, args), indent=2))
```

Use it as: `python3 qa_ipc.py $RUN_DIR/postbloxd.sock account.list`

## Available test flows (menu — pick by diff)

The orchestrator picks only the flows whose label maps to a changed
file/module. Each flow includes the IPC ops it exercises so the
orchestrator can choose accurately.

### read-ops (always run if any daemon code changed)
- Call `account.list` → expect empty array on a fresh DB.
- Call `folder.list` with a synthetic `account_id` → expect empty array.
- Call `audit.list_recent` with `{"limit": 10, "offset": 0}` → expect array.
- Call `search` with `{"q": "anything"}` → expect empty `hits`.

### account-lifecycle (run if `src/db/accounts*`, `src/daemon/**`, or auth code changed)
- `account.create` with valid args → expect a new id back.
- `account.set_secret` with `{"account_id": <id>, "kind": "imap", "secret": "qa-dummy-pass"}` → expect ok.
- `account.delete_secret` with same kind → expect ok.
- `account.delete` → expect ok; subsequent `account.list` is empty again.

### folder-and-message ops (run if `src/db/folders*`, `src/db/messages*`, or `src/mail/**` changed)
- `folder.upsert` with mock IMAP folder shape → expect id.
- `message.list_by_folder` on the folder → expect array.
- `message.list_by_thread` on a synthetic thread id → expect array.
- `message.get` on a known id → expect message body.
- `message.set_flags` with `["Seen"]` → expect ok; re-read shows the flag.

### draft and send (run if `src/db/drafts*`, `src/smtp/**`, or `src/mail/**` changed)
- `draft.create` with `{to_addrs, cc_addrs, bcc_addrs, subject, body}` → expect id.
- `draft.update` → expect ok.
- `draft.delete` → expect ok.
- **Negative:** `message.send` against the dummy SMTP host → expect a
  structured SMTP/connection error in the response, NOT a panic and NOT
  a hung socket. The point is to verify the error path surfaces cleanly.

### sync-ops (run if `src/sync/**`, `src/imap/**`, or `src/daemon/**` changed)
- `account.sync_folder` against the dummy IMAP host → expect a
  structured auth/network error; the daemon must remain alive afterwards
  (verify by re-running `account.list`).
- `account.start_sync` → expect ok.
- `account.stop_sync` → expect ok; subsequent worker registry queries
  show no live worker.

### event-hub (run if `src/ipc/**` or `src/daemon/**` changed)
- Open a second socket connection and subscribe to topic `mail.new`.
- On the first connection, publish a synthetic event by calling an op
  that should fan out (e.g., a write op).
- The second connection should receive the event within 1s. If it doesn't,
  FAIL with the captured log output.

### graceful-shutdown (run if `src/bin/postbloxd.rs`, `src/ipc/**`, or `src/sync/**` changed)
- Send SIGTERM to the pid in `postbloxd.pid`.
- The daemon must exit within 5s with status 0.
- The socket file must be removed.
- If either condition fails, FAIL with the log tail.

### config-load (run if `src/config.rs` or `postblox.toml.example` changed)
- Spawn a second daemon with a deliberately malformed `postblox.toml`.
- Expect non-zero exit and a clear error message in stderr.
- Spawn with a missing passphrase but `backend = "file"`.
- Expect non-zero exit and the "file secrets backend requires
  [secrets] passphrase" error.

### migrations (run if `migrations/**` changed)
- Open the SQLite DB at `$RUN_DIR/postblox.db` after the daemon has
  started.
- Run `sqlite3 "$RUN_DIR/postblox.db" "SELECT name FROM
  sqlite_master WHERE type='table' ORDER BY 1"`.
- Compare against the expected table list (from the most recent
  migration). FAIL on any drift.

### diff-targeted ad-hoc (always)
Read the diff hunks and write 1+ test that directly exercises the changed
code path. Examples:
- New op string added to dispatcher → call it via the helper.
- New field added to a domain type → confirm round-trips through IPC.
- New error variant in `ImapError` / `SmtpError` → trigger it and verify
  the response body contains the new variant tag.

## Per-persona variations

The "user" persona drives daemon QA via the IPC helper above. The
"agent" persona doesn't talk to the daemon directly -- it goes through
the MCP bridge (see `qa-mcp` sub-skill). If a daemon op is reachable
from the MCP bridge AND it changed, run the same op from both sub-skills
and verify the responses agree.

## Evidence

For each flow:
- Emit the request frame as `{op, args}` JSON in a fenced code block.
- Emit the response frame in the next code block.
- For sync/idle/error flows, also include the last 5 lines of
  `$RUN_DIR/postbloxd.log` to prove the daemon stayed alive (or that it
  logged the expected error and exited cleanly).

## Known Failure Modes

1. **Socket never appears.** The most common cause is a malformed
   `postblox.toml`. Inspect `$RUN_DIR/postbloxd.log` for "load config" or
   "file secrets backend requires" errors.
2. **Daemon hangs on `account.sync_folder`.** Should not happen — IMAP
   ops have a deadline. If it does, FAIL the flow and capture stderr; do
   not retry.
3. **Argon2 KDF makes file-secret ops slow.** A first `account.set_secret`
   call can take ~1-2s on cold cache. Subsequent calls are fast. This is
   not a bug.
4. **`audit.list_recent` requires `offset`.** Older docs/snippets may
   pass only `{limit}`; the daemon expects both `limit` and `offset`.
5. **Migrations are forward-only.** A migration rename or rewrite WILL
   break a re-used DB; that's why each run uses a fresh tempdir.
