---
name: qa
description: >
  Run functional QA for postblox. Analyzes the git diff to determine which app
  was touched (daemon, TUI, or MCP bridge), then routes to the matching
  qa-<app> sub-skill. Spawns a fresh local daemon from the PR branch's
  compiled binaries, exercises real keystrokes via tuistory, real JSON
  frames over the Unix socket, and real JSON-RPC over stdio for the MCP
  bridge. Use when verifying a PR, smoke-testing a release, or validating
  that the daemon, TUI, and MCP bridge still work end-to-end.
---

# QA Orchestrator — postblox

**SCOPE: This skill performs manual/functional QA only.** That means: build
the binaries, launch the daemon, drive the TUI as a real user (tuistory),
call the MCP bridge as a real agent (stdio JSON-RPC), and call the daemon
directly over its Unix socket (length-prefixed JSON frames).

**Do NOT run or report on:** `cargo test`, `cargo clippy`, `cargo fmt`,
`cargo deny`, `cargo audit`, `cargo doc`, coverage, MSRV checks. The repo's
CI workflow (`.github/workflows/ci.yml`) already covers all of those on
every PR. This skill exists to verify the *behaviour* a user or AI agent
sees, not the static checks.

## Step 1: Load Configuration

Read `.factory/skills/qa/config.yaml`. It is the single source of truth for
the project, persona definitions, app routing, and cleanup strategy.

## Step 2: Determine Target Environment

postblox is a local-first single-user app. **There is no remote
environment.** The default target is always a freshly-spawned local daemon
under `./qa-results/<RUN_ID>/`. The agent MUST honour the `restrictions`
list in `config.yaml`:

- Never connect to a real IMAP/SMTP host (`imap.gmail.com`,
  `smtp.fastmail.com`, etc.)
- Never write to the user's real data dir (`~/.local/share/postblox`)
- Always set `POSTBLOX_DB`, `POSTBLOX_SOCKET`, `POSTBLOX_CONFIG`,
  `XDG_DATA_HOME`, `XDG_RUNTIME_DIR` to point inside the run-scoped tempdir

## Step 3: Analyze Git Diff

Run `git diff origin/main...HEAD --name-only` (fall back to `git diff
HEAD~1 --name-only` if the merge base is unknown).

Map each changed file to an app using `apps.<name>.path_patterns` in
`config.yaml`. Files that match nothing (e.g., `.factory/skills/**`,
`docs/**`, `.github/**`, `Cargo.toml`, `*.md`, `clippy.toml`,
`rust-toolchain.toml`) are NOT associated with any app.

For each affected app:
- Read its sub-skill at `.factory/skills/qa-<app-name>/SKILL.md`
- Run only that sub-skill's flows that match the changed surface
- Generate at least one diff-targeted ad-hoc test that directly exercises
  the changed code path

For apps NOT affected by the diff:
- Do NOT load their sub-skill, do NOT run their flows, do NOT run their
  pre-flight checks. The diff determines scope, period.

If NO app is affected by the diff (e.g., docs-only, CI-only, config-only,
or skill-only changes), report INCONCLUSIVE: "No app code changed -- QA
not applicable for this diff." Do NOT run any app flows.

**Cross-app rule.** Some changes touch shared modules (`src/db/**`,
`src/ipc/**`, `src/models.rs`). Treat these as affecting **all three** apps
(daemon, TUI, MCP) because they're on the wire path between them.

## Step 4: Pre-flight Checks (per affected app)

Before running flows for a given app:

| App | Pre-flight | Failure handling |
|---|---|---|
| daemon | `cargo build --bin postbloxd --locked` succeeds; binary exists at `target/debug/postbloxd` | If build fails, report all daemon flows as BLOCKED with the cargo error |
| tui    | `cargo build --bin postblox --bin postbloxd --locked` succeeds; `tuistory --version` returns successfully | If build fails or `tuistory` is missing, BLOCKED |
| mcp    | `cargo build --bin postblox-mcp --bin postbloxd --locked` succeeds | If build fails, BLOCKED |

For all apps:
- Create the run tempdir `./qa-results/<RUN_ID>/`
- Write a minimal `postblox.toml` with `[secrets] backend = "file"` and a
  dummy passphrase so no OS keyring is touched
- Spawn `postbloxd` with the env overrides from `config.yaml` and capture
  the pid in `./qa-results/<RUN_ID>/postbloxd.pid`
- Wait up to 3s for the socket to appear; if it doesn't, BLOCKED

## Step 5: Execute Diff-Relevant Flows Only

For each affected app, the sub-skill exposes a **menu** of flows. The
orchestrator picks:

1. Every flow whose label maps to a changed file/module in the diff
2. Adjacent integration flows that verify the change didn't break a
   neighbouring surface (e.g., a new TUI command should also appear in
   `:` Tab-completion; a new daemon op should be reachable from both the
   TUI and the MCP bridge)
3. At least one **negative** test for the changed area (invalid input,
   missing arg, gate=deny, malformed JSON frame)
4. At least one **diff-targeted ad-hoc** test that directly exercises the
   code path that actually changed (read the diff hunks, not just the
   filenames)

Do NOT run completely unrelated flows. Do NOT run `cargo test`. Do NOT
run `cargo clippy`. Do NOT report on lint or formatting.

## Step 6: Evidence Capture

Use **text snapshots as primary evidence** -- they render inline in the PR
comment with no image hosting issues.

For the TUI (tuistory):
- Use `tuistory -s qa-test snapshot --trim` to capture terminal state as
  text. Each snapshot MUST show something visibly different from the
  previous one. Wait for the UI to settle before capturing.
- Save full PNG snapshots to `./qa-results/<RUN_ID>/snapshots/` for the
  artifact upload.
- If ImageMagick is available (config: imagemagick=true), generate an
  animated GIF diff of before/after states for any visual regression test.

For the daemon (Unix socket):
- Capture the request/response JSON frames as text. Format with `jq -C`
  if available, else raw.
- Embed the relevant frames (request + response) directly in the report
  as a fenced code block.

For the MCP bridge (stdio JSON-RPC):
- Capture the line-delimited JSON-RPC request/response pairs.
- Embed them in the report. Trim to the meaningful fields (method,
  params summary, result summary, errors).

Evidence quality rules:
- Trim snapshots to the meaningful part. Don't dump the whole 110x36 grid
  if the change is in a single pane.
- Label each snapshot clearly: what it shows and why it matters.
- NEVER embed `![image](url)` markdown -- those URLs won't resolve in PR
  comments. Use text snapshots inline; reference PNG/GIF filenames for
  the downloadable artifact.

## Step 7: Test Quality Gate

Before generating the report, the agent MUST verify:

1. **Change-specific first.** At least 50% of the test rows directly
   verify behaviour from the diff hunks.
2. **Integration tests are valid.** Tests that verify the change didn't
   break a neighbouring surface (e.g., a new TUI command also appears in
   `:` Tab-completion, a new daemon op is callable from MCP) are good.
3. **No unrelated flows.** Do NOT test compose when only `:theme` changed.
   Do NOT test MCP when only `src/sync/**` changed (unless the sync
   change affects an MCP-visible op).
4. **No automated test suites.** Do NOT run `cargo test`, `cargo clippy`,
   `cargo fmt`, `cargo deny`, `cargo audit`, `cargo doc`, or `coverage`.
5. **At least one negative test.** Bad input, missing required field,
   gate=deny, etc.
6. **Real interaction.** Use real keystrokes (TUI), real JSON frames
   (daemon), real JSON-RPC lines (MCP). No "I assume this works" rows.
7. **INCONCLUSIVE if unsure.** If the agent cannot articulate what the
   diff changes, mark INCONCLUSIVE rather than PASS.

## Step 8: Handle Failures

**Never silently skip a flow.** If a flow cannot complete, report it as
BLOCKED with what was tried and how the user can fix it. Then continue
to the next flow -- never abort the entire run for a single failure.

Specific failure modes:
- Build failure → all flows for that app BLOCKED with cargo output
- `tuistory` not installed → all TUI flows BLOCKED with install command
  (`npm install -g tuistory`)
- Socket never appears within 3s → daemon BLOCKED; capture stderr from
  the daemon log at `./qa-results/<RUN_ID>/postbloxd.log`
- TUI panic → BLOCKED; capture the last 20 lines of the TUI snapshot and
  the panic message from stderr

## Step 9: Generate Report

Generate the report at `./qa-results/report.md` using
`.factory/skills/qa/REPORT-TEMPLATE.md`.

The report MUST follow the template. Key rules:

- Start with `## QA Report` heading followed by the test results table.
- Result column MUST use emojis: :white_check_mark: PASS, :x: FAIL,
  :no_entry: BLOCKED, :warning: FLAKY, :grey_question: INCONCLUSIVE.
- Keep it concise. Table + short Action Required + collapsed evidence =
  the entire report.
- Do NOT include verbose explanations of what the diff does -- the
  reviewer already knows.
- Do NOT report setup steps (build, daemon spawn) as test rows. Those
  are means to an end. Only report rows that verify user-facing
  behaviour or the specific behavioural change in the diff.
- Put ALL evidence in a single collapsed `<details>` block at the bottom.
- For TUI evidence: embed `tuistory snapshot --trim` output as labelled
  fenced code blocks.
- For daemon/MCP evidence: embed the JSON request/response frames.
- Reference PNG/GIF filenames in the artifact for visual proof; do NOT
  use `![image]()` markdown.

## Step 10: Suggest Skill Updates (Failure Learning)

`failure_learning: suggest_in_report` (from config.yaml).

After generating the report, check if any BLOCKED or FAIL rows revealed a
**testing-environment insight** that would help future QA runs succeed.

**Good suggestions** (environment/workflow knowledge worth recording):

- "tuistory needs `--cols 110 --rows 36` for the TUI's three-pane layout
  to render; smaller terminals collapse panes and break the snapshot
  match"
- "The daemon's IPC handshake takes ~200ms on cold start; bump the
  socket-wait timeout from 3s to 5s on slow CI runners"
- "When `[secrets] backend = \"file\"` is used, the daemon refuses to
  start without a non-empty passphrase -- always set one in the QA
  config even if it's a dummy"

**Bad suggestions** (skill bugs, not environment insights -- do NOT
suggest these):

- "Selector `.message-list-item` not found" -- this is the TUI; there are
  no selectors. If the snapshot doesn't match, fix the snapshot.
- "The button text changed from X to Y" -- that's expected from the diff.
- "`cargo test` failed" -- not a QA concern; CI handles it.

Format as a table with severity and a copy-pasteable fix prompt:

```markdown
## Suggested Skill Updates (N issues found)

| # | Severity | File | Issue | Fix Prompt |
|---|---|---|---|---|
| 1 | 🟡 Degraded | `.factory/skills/qa-tui/SKILL.md` | tuistory cold-start timeout | <details><summary>Copy</summary><br>`Bump the socket-wait timeout in qa-tui SKILL.md "Pre-flight checks" from 3s to 5s.`</details> |
```

Severity levels:
- `🔴 Breaking` -- causes failures every run (wrong path, missing env var, missing required step)
- `🟡 Degraded` -- causes intermittent failures (timing, locale, layout sensitivity)
- `🔵 Info` -- new knowledge that improves future runs (new pattern, new endpoint)

Each Fix Prompt must be a self-contained instruction Droid can execute
when pasted. Do NOT write `qa-results/skill-updates.json` -- that file is
only for `auto_commit` / `open_pr` modes. This project uses
`suggest_in_report` only.

If no genuinely new environment insight was discovered, omit the section
entirely.

## Cleanup

After the report is written, run the cleanup steps from `config.yaml`:
1. SIGTERM postbloxd; wait up to 5s; SIGKILL if still alive
2. Remove the socket file
3. Remove `<RUN_ID>/postblox.db` and `<RUN_ID>/data/` (keep snapshots,
   logs, report.md for the artifact upload)
