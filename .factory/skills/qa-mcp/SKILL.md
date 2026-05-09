---
name: qa-mcp
description: >
  Functional QA tests for the postblox MCP bridge (the `postblox-mcp`
  binary, a JSON-RPC stdio shim that exposes 12 tools backed by the
  daemon's IPC ops, plus a notifications stream and per-tool /
  per-pattern approval gates). Used by the qa orchestrator when the
  diff touches src/bin/postblox-mcp.rs or src/mcp/**.
---

# qa-mcp — postblox MCP bridge functional QA

## Testing Target

The MCP bridge is a stdio shim: stdin/stdout speak line-delimited
JSON-RPC. It connects to a postbloxd over the Unix socket pointed to
by `POSTBLOX_SOCKET`. **Always test against a fresh daemon + fresh
bridge built from the PR branch.**

## Authentication in CI

No external authentication. The MCP bridge talks to the same
file-backed-secret daemon as `qa-daemon`. There are no GitHub secrets
needed for MCP QA.

## Pre-flight checks

1. `cargo build --bin postblox-mcp --bin postbloxd --locked` — must succeed.
2. Spawn the daemon (same procedure as `qa-daemon` Pre-flight).
3. Confirm `target/debug/postblox-mcp` exists.

## How to talk to the MCP bridge

Spawn `postblox-mcp` as a subprocess with its stdin/stdout connected to
your test harness. The protocol is line-delimited JSON-RPC 2.0 with a
trailing newline per message.

Inline Python helper for JSON-RPC over stdio:

```python
# qa_mcp.py
import json, subprocess, threading, queue, os, sys

class McpClient:
    def __init__(self, socket_path, binary="./target/debug/postblox-mcp"):
        env = os.environ.copy()
        env["POSTBLOX_SOCKET"] = socket_path
        env["RUST_LOG"] = "warn"
        self.p = subprocess.Popen(
            [binary],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self.q = queue.Queue()
        threading.Thread(target=self._reader, daemon=True).start()
        self.next_id = 1

    def _reader(self):
        for line in self.p.stdout:
            line = line.decode().strip()
            if line:
                self.q.put(json.loads(line))

    def send(self, method, params=None, request_id=None):
        if request_id is None:
            request_id = self.next_id
            self.next_id += 1
        msg = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}}
        self.p.stdin.write((json.dumps(msg) + "\n").encode())
        self.p.stdin.flush()
        return request_id

    def recv(self, timeout=5.0):
        return self.q.get(timeout=timeout)

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=5)
```

The 12 tools (from `src/mcp/tools.rs`):
1. `postblox_account_list`
2. `postblox_folder_list`
3. `postblox_thread_list`
4. `postblox_message_list_by_folder`
5. `postblox_message_list_by_thread`
6. `postblox_message_get`
7. `postblox_search`
8. `postblox_draft_create` (dangerous)
9. `postblox_draft_update` (dangerous)
10. `postblox_draft_delete` (dangerous)
11. `postblox_message_send` (dangerous)
12. `postblox_message_set_flags` (dangerous, expected — confirm against `tools.rs`)

Re-read `src/mcp/tools.rs` at the start of every run; the list and
`dangerous` flags are the source of truth.

## Available test flows (menu — pick by diff)

### initialize-and-list (always run if any MCP code changed)
- Send `initialize`; expect a result with capabilities and protocol
  version.
- Send `tools/list`; expect exactly 12 entries with the tool names from
  `src/mcp/tools.rs`. Any drift (extra, missing, renamed) → FAIL.
- Send `tools/list` a second time; result must be deterministic.

### read-only tools (run if `src/mcp/tools.rs` or `src/mcp/server.rs` changed)
For each non-dangerous tool, call `tools/call` and verify:
- The op string in the response matches the spec.
- Required args are enforced — calling with no args returns a JSON-RPC
  error with `code: -32602` (Invalid params) or the project's
  equivalent.
- The shape of the `result.content` matches the schema in `tools.rs`.

### dangerous tools under each gate mode (run if `src/mcp/gates.rs` or
### `src/mcp/server.rs` changed)
Configure the daemon with three different gate setups via
`postblox.toml` and restart. For each setup, attempt a dangerous tool
call and verify the gate engine's behaviour:

- `default = "auto_allow"` → call goes through; daemon receives the op.
- `default = "require"` → call is queued for approval; the bridge
  surfaces a "pending approval" notification or response. The QA flow
  then approves it via the documented mechanism (read `src/mcp/gates.rs`
  to discover the approval surface) and verifies the op completes.
- `default = "deny"` → call is rejected immediately with a structured
  error; the daemon never receives the op (verify by checking the
  daemon log).

Per-tool / per-pattern overrides:
- Set `[[mcp.gates.rule]] tool = "send" to = "*@allowed.test" action =
  "auto_allow"` and `default = "deny"`.
- Call `postblox_message_send` with `to = "qa@allowed.test"` → expect
  auto_allow.
- Call `postblox_message_send` with `to = "qa@blocked.test"` → expect
  deny.

### notifications-stream (run if `src/mcp/server.rs` or notifications
### code changed)
- After `initialize`, send a daemon write op via `qa-daemon`'s helper
  that triggers a `mail.new` event on the daemon's hub.
- The MCP bridge must forward the event to stdout as a `notifications/`
  JSON-RPC message within 2s.
- Verify the notification's params include the event payload.

### error handling (run if any MCP code changed)
- Send a malformed JSON line → bridge MUST NOT crash; expect a
  JSON-RPC parse error response.
- Send a `tools/call` for a non-existent tool → expect a structured
  "method not found" error.
- Send a `tools/call` with the wrong arg type (e.g., string where int
  expected) → expect "invalid params".
- Send 100 calls in rapid succession → all complete; no responses are
  dropped.

### shutdown (run if `src/bin/postblox-mcp.rs` or stdio handling changed)
- Close the bridge's stdin → bridge exits within 2s with status 0.
- Send SIGTERM → bridge exits within 2s with status 0.
- The daemon must remain alive after the bridge exits.

### diff-targeted ad-hoc (always)
Read the MCP diff hunks and write at least one test that directly
exercises the changed code path. Examples:
- New tool added to `TOOLS` array → `tools/list` includes it; calling
  it routes to the right daemon op.
- New gate rule pattern → exercise the new pattern with both matching
  and non-matching args.
- New notification type → trigger it on the daemon and verify the
  bridge forwards it.

## Per-persona variations

This sub-skill exists for the "agent" persona. The "user" persona does
not touch the MCP bridge; skip these flows when only the user
persona's surface changed.

## Evidence

For each flow:
- Embed the JSON-RPC request line in a fenced code block.
- Embed the JSON-RPC response (or notification) in the next code block.
- For gate flows, also include a one-line summary: "gate=auto_allow,
  expected=allowed, observed=allowed".

## Known Failure Modes

1. **`tools/list` count drift.** If the count isn't exactly 12, somebody
   changed the tool set. That requires a sub-skill update too — flag in
   the Suggested Skill Updates section.
2. **Bridge hangs on close if daemon is down.** The bridge tries to
   flush the IPC connection. If the daemon is already gone, it can
   block briefly. Wait up to 5s before SIGKILL.
3. **`require` gate has no QA hook.** If the gate engine in this branch
   doesn't expose a programmatic approval API, mark the `require` flow
   as INCONCLUSIVE (NOT FAIL) and add a Suggested Skill Update asking
   for one.
4. **`deny` action returns an error, not a refusal.** The structured
   response shape may surface as a JSON-RPC error rather than a
   `result.refused: true` payload. Read `src/mcp/server.rs` for the
   actual shape and assert against that.
5. **Pattern matching is case-sensitive.** Gate rules with
   `to = "*@MyOrg.com"` will NOT match `qa@myorg.com`. The QA
   flow must use the exact case from the rule.
