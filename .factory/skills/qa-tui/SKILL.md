---
name: qa-tui
description: >
  Functional QA tests for the postblox TUI (the `postblox` binary, a
  ratatui app that talks to postbloxd over a Unix socket). Builds the
  binary, spawns the daemon in the background, launches the TUI inside
  tuistory, and exercises navigation, commands, compose/reply/forward,
  search, drafts, attachment preview, themes, and the command bar.
  Used by the qa orchestrator when the diff touches src/main.rs or
  src/tui/**.
---

# qa-tui — postblox TUI functional QA

## Testing Target

postblox is a local-first single-user app — there are no preview
deployments. **Always test against a freshly-built TUI driving a
freshly-spawned daemon, both built from the PR branch.** Do NOT test
against a system-installed `postblox` binary.

If the host has no `tuistory` available, report ALL TUI flows as BLOCKED
with the install command (`npm install -g tuistory`). Do NOT fall back
to "I read the source and it looks right" — that is not QA.

## Authentication in CI

No external authentication. The daemon uses `[secrets] backend = "file"`
with the dummy passphrase `qa-dummy-pass` (same as `qa-daemon`). There
are no GitHub secrets needed for TUI QA.

## Pre-flight checks

1. `cargo build --bin postblox --bin postbloxd --locked` — must succeed.
2. `tuistory --version` — must succeed. If not installed, BLOCKED.
3. Spawn the daemon (same procedure as `qa-daemon` Pre-flight steps 2-5).
4. Confirm `target/debug/postblox` exists and is executable.

## How to drive the TUI

Use the `droid-control` skill for all tuistory interactions. The
droid-control skill contains the complete tuistory API reference. Do
NOT write raw tuistory commands here.

In CI, prefix every launch with `env -u CI FACTORY_DISABLE_KEYRING=true`
to avoid Ink's CI auto-detect. Use session name `-s qa-test` and
`--cols 110 --rows 36` (the TUI's three-pane layout collapses below
~100 cols).

The TUI's startup sequence:

1. Reads `POSTBLOX_SOCKET` and `POSTBLOX_CONFIG`.
2. Loads the theme from `[tui] theme = "..."` in `postblox.toml` (or
   `dark` by default).
3. Connects to the daemon over the Unix socket.
4. Renders three panes: account list, folder list (or thread list,
   depending on the focus), and message detail.
5. Reads keystrokes; `:` enters command bar; `?` shows help; `q` quits.

If the daemon socket is missing, the TUI prints an error and exits
non-zero. The qa-tui skill MUST verify the TUI starts cleanly when the
socket is present and that it surfaces a helpful error when it isn't.

## Available test flows (menu — pick by diff)

### startup (always run if any TUI code changed)
- Launch `postblox` with the live socket → expect three-pane layout
  visible within 2s. Snapshot the initial frame.
- Launch with `POSTBLOX_SOCKET=/tmp/does-not-exist` → expect non-zero
  exit and a "could not connect" or similar error in stderr.

### navigation (run if `src/tui/app.rs` or `src/tui/mod.rs` changed)
- `j` / `k` and arrow keys cycle through items in the focused pane;
  capture before/after snapshots showing the highlight moved.
- `Tab` / `Shift-Tab` rotates pane focus; verify the focus border moves.
- `Enter` opens the focused thread/message; capture the detail snapshot.
- `Esc` returns to the previous pane; snapshot.
- `q` quits cleanly; verify exit code 0.

### message-detail (run if `src/tui/render.rs` or attachment code changed)
- Open a synthetic message (insert via the daemon first, then refresh
  the TUI with `:sync` or `r`).
- Verify From/To/Subject/Date render. Snapshot.
- Trigger attachment preview if the message has one; verify the preview
  pane appears and is scrollable with `j`/`k`.
- Trigger copy-to-clipboard with the documented binding; verify the
  system clipboard contains the expected text (use `xclip -o` on Linux).

### command-bar (run if `src/tui/command.rs` changed)
- Press `:` → command bar appears.
- Tab-completion: type `:s` then Tab → expect cycle through `seen`,
  `start-sync`, `stop-sync`, `search`, `sync` in that exact alphabetical
  order from `COMMAND_NAMES`.
- Type `:doesnotexist` and Enter → expect "unknown command 'doesnotexist'"
  status row.
- Type `:theme` (no arg) → expect a usage error.
- Type `:theme dark` and Enter → status row clears, theme switches.

### theme-switch (run if `src/tui/theme.rs` or `[tui] theme` changed)
- Default theme matches the value in `[tui] theme = ...` from
  `postblox.toml`.
- `:theme dark`, `:theme light`, `:theme high-contrast`/`:theme hc`
  each switch the theme; snapshot before/after to prove colours change.
- **Negative:** `:theme nope` → expect "unknown theme" status row;
  current theme unchanged.

### compose-reply-forward (run if `src/tui/app.rs` compose paths,
### `src/mail/**`, or `src/db/drafts*` changed)
- `:compose` → composer opens. Type a To, Subject, body. `Ctrl-S` /
  `:w` saves the draft. Snapshot.
- Quit composer; verify the draft appears in the drafts folder via
  `:goto drafts` or equivalent.
- Reopen the draft; verify fields are populated correctly.
- `:reply` on a synthetic message → composer opens prefilled with
  In-Reply-To and a quote block.
- `:reply-all` → To and Cc both populated.
- `:forward` → composer opens with `Fwd:` subject and original body
  quoted.
- Add an attachment via the documented compose binding; verify the
  attachment count appears in the composer header.
- **Negative:** save a draft with empty To and try to send → expect a
  validation error in the status row, NOT a daemon round-trip.

### search-ui (run if `src/tui/app.rs` search paths or `src/db/search*` changed)
- `:search hello` → search pane appears with FTS5 hits (empty on a
  fresh DB; insert a synthetic message via the daemon first to get
  hits).
- Selecting a hit opens the message detail. Snapshot.
- Re-running the same search returns the same hits in the same order.

### draft-list (run if `src/tui/app.rs` drafts panes or `src/db/drafts*` changed)
- `:goto drafts` → drafts list pane appears.
- Open a draft, edit, save, close; verify the list still shows it.
- Delete a draft; verify it disappears.

### sync-and-stop-sync (run if `src/tui/app.rs` sync hooks or `src/sync/**` changed)
- `:start-sync` → status row shows "starting sync".
- `:stop-sync` → status row shows "sync stopped".
- `:sync` → triggers a one-shot sync. Against the dummy IMAP host this
  must surface a clean error in the status row, NOT a TUI panic.

### diff-targeted ad-hoc (always)
Read the TUI diff hunks and write at least one test that directly
exercises the changed code path. Examples:
- New keybinding → press it in the matching pane and verify the result.
- New status-row message → trigger it and snapshot.
- New theme → switch to it and snapshot.

## Per-persona variations

The "user" persona drives the TUI. The "agent" persona does NOT touch
the TUI -- skip TUI flows when only the agent persona's surface changed.

## Evidence

- For every state-changing flow, capture `tuistory snapshot --trim`
  before AND after the change. Embed both as labelled fenced code
  blocks in the report.
- Save the full snapshot PNGs to `./qa-results/<RUN_ID>/snapshots/` for
  the artifact upload.
- If `magick`/`convert` is available (config: imagemagick=true) and the
  flow is a visual change (theme, status row, pane focus), generate an
  animated GIF diff and reference it from the report.

## Known Failure Modes

1. **Three-pane layout collapses on small terminals.** Always launch with
   `--cols 110 --rows 36`. If a snapshot shows fewer panes than
   expected, check the terminal size first before reporting a layout
   bug.
2. **Theme defaults to "dark" if `[tui]` is missing.** When testing the
   "default theme" flow, do NOT assume `light` -- always read what's in
   the run's `postblox.toml`.
3. **Tab-completion order is alphabetical.** The `COMMAND_NAMES` slice
   is sorted; tests that assert order must use the alphabetical order,
   not insertion order.
4. **`Ctrl-S` and `:w` are aliases.** Tests for "save draft" must accept
   either as the trigger; don't hard-code one.
5. **Quitting hangs if the daemon socket disappears mid-run.** The TUI
   tries to reconnect. Send SIGTERM after 2s if `q` doesn't take effect.
6. **Clipboard tests fail in headless CI without xvfb.** When `arboard`
   is built but no display server is running, the clipboard write/read
   round-trip surfaces an error. Either skip the copy-to-clipboard
   assertion in CI or run under `xvfb-run`.
