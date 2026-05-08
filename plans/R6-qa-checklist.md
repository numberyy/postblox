# R6 — Manual QA Checklist

Run this from a clean clone with at least one configured account
(Personal account recommended; Gmail OAuth not required). `tuistory`
would normally automate this — until that skill is available in this
environment, run by hand and tick the boxes as you go.

Notation:

- `<…>` are placeholders you fill in from your environment.
- `Ctrl-X` means hold `Ctrl` and press `X`.
- "the messages list" = the upper-right pane (or the lower-right pane
  in narrow layouts) once an account/folder is selected.

## Setup

- [ ] Build: `cargo build --release`
- [ ] Start daemon: `./target/release/postbloxd`
- [ ] Confirm the log line `listening` is printed with a socket path
      (default: `$XDG_RUNTIME_DIR/postblox.sock`).
- [ ] In a second terminal start TUI: `./target/release/postblox`
- [ ] Status line shows `Connected to <socket-path>`, panes load with
      at least one account.
- [ ] At least one folder loads under the selected account, with one
      or more messages in it.

## Slice 1 — Write-through message actions

- [ ] In INBOX, focus the messages pane (`Tab` / `→` until the title
      bar highlights it). Press `e`. → status flashes `Archiving…`,
      then settles on `Archived`; the row disappears from the list.
- [ ] Select another message, press `d`. → modal/status `Delete? y/n`.
      Press `n` → `Delete cancelled`, row stays.
- [ ] Repeat `d`, this time press `y`. → status `Deleting…` → `Deleted`,
      row disappears.
- [ ] Select another message, type `:move INBOX/Receipts 2025` Enter
      (folder name with a space and a slash must survive). → status
      `Moving to INBOX/Receipts 2025…` → `Moved to INBOX/Receipts 2025`.
- [ ] Press `m` (with messages pane focused). → command bar opens
      pre-filled with `move ` (cursor at end). Backspace and `Esc`
      both leave you in command mode without crashing.
- [ ] Press `u`. → toggles `\Seen` flag. Status reports
      `Marked message seen` / `Marked message unseen`. Row's read/unread
      indicator updates.
- [ ] Press `f`. → toggles `\Flagged`. Status reports
      `Flagged message` / `Unflagged message`.
- [ ] Press `*`. → also toggles `\Flagged` (parity with `f`). Row's
      flagged indicator updates.
- [ ] Force a daemon error (e.g. point the TUI at a stopped daemon, or
      kill the daemon between sending the op and the reply). → status
      surfaces an error string, list rolls back: the optimistically
      removed row is restored.

## Slice 2 — Sync state, toasts, status pane

- [ ] Start a sync: with the messages pane focused press `s` (or run
      `:sync`). → status flashes `Syncing <folder>` then `Synced
      <folder>`.
- [ ] While sync is running, watch the status line's per-account
      prefix. → cycles through `~ <label>` (polling) → `… <label>`
      (syncing) → `● <label>` (idle). Multiple accounts join with
      ` · `.
- [ ] Receive a new message (or trigger one with `:sync` against a
      folder that has new mail). → toast `New mail in <folder>
      (<account>)` appears at the bottom. Stack cap is 3; older toasts
      drop when a 4th lands.
- [ ] Press `x` while a toast is visible. → newest toast dismisses.
- [ ] Press `X`. → all visible toasts cleared.
- [ ] Force a sync error (e.g. set an obviously wrong password in
      `postblox.toml`, restart daemon, run `:sync`). → status pane icon
      switches to `! <label>: <error>` for the affected account, and a
      red toast surfaces the error message. Repeated identical errors
      coalesce instead of stacking 3 deep.
- [ ] After ~3 seconds of inactivity, info toasts time out; error
      toasts stick around longer (~10 s).

**Known limitations**

- Sync state cycles purely off `sync.state` events from the daemon; if
  the daemon never publishes a transition, the icon stays at its last
  known value. There is no client-side timeout that will reset it.

## Slice 3 — Command bar parity

For every command below: open the command bar with `:`, type the
command line, press Enter. The behaviour must match the equivalent
keybinding exactly (same handler, no duplicated logic).

- [ ] `:compose` → opens the composer for the currently selected
      account (same as `c`).
- [ ] `:reply` → opens the composer pre-filled as a reply (same as `R`).
- [ ] `:reply-all` → reply-all (same as `A`).
- [ ] `:forward` → forward (same as `F`).
- [ ] `:archive` → archives the selected message (same as `e` in the
      messages pane).
- [ ] `:delete` → arms the `Delete? y/n` confirmation (same as `d`).
- [ ] `:move INBOX/Receipts 2025` → moves the selected message,
      keeping the multi-word folder name intact.
- [ ] `:goto Sent  Items` → switches to a folder whose name has internal
      whitespace; multi-word folder names are preserved.
- [ ] `:account Personal` (or `:account you@example.com`) → switches
      account by case-insensitive label OR email match. Folder/message
      lists refresh.
- [ ] `:account NoSuch` → status + error toast `No account named
      'NoSuch'`.
- [ ] `:search foo bar baz` → runs an unscoped search across all
      accounts (same as `/`).
- [ ] `:search --account Personal foo bar` → runs a search scoped to
      the named account.
- [ ] `:search --account NoSuch foo` → toast + error
      `unknown account: NoSuch`. Search pane does not open.
- [ ] `:theme dark`, `:theme light`, `:theme high-contrast`, `:theme hc`
      → each switches palette in-memory (status `Theme: <name>`).
- [ ] `:theme next` → cycles `light → dark → high-contrast → light`.
- [ ] `:theme bogus` → command-bar error `usage: theme
      next|light|dark|high-contrast`.
- [ ] `:w` while composing → saves the draft (alias for `Ctrl-S`).
- [ ] `:w` outside the composer → status `:w only valid while
      composing`. No daemon op fires.
- [ ] Tab autocompletion: type `:m<Tab>`. → command bar resolves to
      `move ` (unique match expands and adds a trailing space).
- [ ] `:g<Tab>` → resolves to `goto `.
- [ ] `:s<Tab>` → ambiguous (search/seen/start-sync/stop-sync/sync);
      the status hint shows `Matches: search seen start-sync stop-sync
      sync` and the input expands to the longest common prefix (`s`).
- [ ] `:<Tab>` (Tab on empty input) → no-op; command bar stays as `:`.
- [ ] `:` at an empty Enter → silent no-op (no error toast, no
      `unknown command`).
- [ ] `:delete-everything` → command-bar error `unknown command
      'delete-everything'`.

**Known limitations**

- `:account "Work Personal"` (quoted multi-token name) is **not**
  supported today; the parser splits on whitespace and rejects extra
  tokens with `usage: account <name|email>`. This is documented in
  `tui::command::test_parse_command_account_quoted_form_is_unsupported_today`.
- `:` typed inside the composer body inserts a literal `:`. Outside
  the body (e.g. `To`/`Cc`/`Subject` fields) it opens the command bar.

## Slice 4 — Theme palettes

- [ ] Ship a `postblox.toml` with `[tui]\ntheme = "dark"`. Restart the
      TUI. → first frame renders in dark mode (visibly different
      background to the default light).
- [ ] `[tui] theme = "high-contrast"` (or `"hc"`) → renders the
      high-contrast palette at startup.
- [ ] `[tui] theme = "solarized"` (invalid) → daemon log emits a
      warning, TUI starts in the default light palette without
      crashing.
- [ ] `:theme light` / `:theme dark` / `:theme high-contrast` /
      `:theme hc` → all four switch immediately and persist for the
      session (config not rewritten — verify by quitting and
      relaunching: theme returns to the configured default).
- [ ] Press `t` (no command bar) → cycles `light → dark →
      high-contrast → light`. Status reports the new theme name.
- [ ] In each palette: focused pane border is visibly bolder than
      inactive panes; selected list row is legible against the
      background; status line and command line use distinct colours.

## Slice 5 — Search UI

- [ ] Press `/` from normal mode. → command line / search input shows
      `/`. Type `meeting`, Enter. → toast `Searching…`, then a Search
      pane opens with hits and toast `<n> result(s)`. Status `<n>
      search result(s)`.
- [ ] With the Search pane focused, `j`/`k` and `↓`/`↑` move the
      selection without leaving the pane.
- [ ] Press Enter on a hit. → TUI switches to the hit's account,
      focuses its folder, loads the message detail. Status `Opened:
      <subject>`.
- [ ] Press `Esc` while the Search pane is focused. → pane closes,
      status `Search closed`. Press `/` again → input opens fresh.
- [ ] `/foo` then immediate Enter on an empty selection → no-op (does
      not open Search if `/` was followed by Enter with no chars).
- [ ] `:search foo` → unscoped search.
- [ ] `:search --account Personal foo bar` → scoped search; pane
      title shows the scope label and hit count.
- [ ] `:search --account Bogus foo` → toast + error `unknown account:
      Bogus`; Search pane does not open.
- [ ] `:search` with no query → command-bar error `usage: search
      [--account <name>] <query>`.
- [ ] With Search pane focused, press `r` → re-runs the active search
      (status shows `Searching <q>` then result count). Used to refresh
      after editing/deleting messages.

## Slice 6 — Compose attachments (outgoing)

- [ ] Open composer (`c` or `:compose`). Press `Tab` until you can
      type a message body, fill in `To`, `Subject`, body.
- [ ] Press `Ctrl-A`. → attach-path mode opens; status `Attach: type a
      path, Enter to add, Esc to cancel`. Type the absolute path of a
      small file (e.g. `/etc/hostname`), Enter. → status `Attached
      <name>`. Attachments panel below the body shows the row.
- [ ] Repeat with a second small file. → both rows visible; running
      total + count shown.
- [ ] Create a 30 MiB file: `dd if=/dev/zero of=/tmp/big.bin bs=1M
      count=30`. `Ctrl-A`, type `/tmp/big.bin`, Enter. → red error
      toast `Attachment too large: 30 MiB > 25 MiB`; attachment is
      **not** added to the panel.
- [ ] Add two small files, then add a third whose size pushes the
      aggregate over 25 MiB. → toast `Aggregate over limit: <total>
      > 25 MiB`; third file rejected.
- [ ] `Ctrl-A`, type a non-existent path, Enter. → toast `File not
      found: <path>`.
- [ ] `Ctrl-A`, type a directory path. → toast `Not a regular file:
      <path>`.
- [ ] Move selection in the attachments panel (focus moves with `Tab`
      to the attachments field; arrows / `j`/`k` change selection).
- [ ] With an attachment selected, press `Ctrl-K`. → row disappears,
      status `Removed attachment: <name>`.
- [ ] `Ctrl-K` with no attachments → status `No attachment to remove`.
- [ ] Save with `Ctrl-S`. → status `Draft saved <uuid>`. Reopen the
      draft (Slice 8 below) and confirm attachments survive.
- [ ] Send with `Ctrl-X`. → status `Sent message <id>`. On the wire
      (e.g. `mailpit`) the email is `multipart/mixed` with each
      attachment as a part with `Content-Disposition: attachment;
      filename="…"`.

**Known limitations**

- The 25 MiB cap is enforced both per-file and against the running
  aggregate. The client-side check fires before the daemon's; the
  daemon also rejects with `attachment_too_large` if the cap is
  exceeded after-the-fact.
- The composer rematerialises attachments to a temp file (under
  `$TMPDIR/postblox-drafts` or `…/postblox-forward`) on reopen — these
  files are not auto-cleaned today.

## Slice 7 — Reply / Reply-all / Forward

- [ ] Select a message with at least one `Cc` recipient and a
      `Message-ID`. Press `R`. → composer opens with `To` filled with
      the original `From`, `Subject` prefixed `Re:` (no double-prefix
      if the original already had `Re:`), body shows the quoted
      original (line-prefixed). Headers `In-Reply-To` and `References`
      are populated and survive a save (`Ctrl-S` then reopen via
      Drafts).
- [ ] Press `A` instead. → reply-all: `To` keeps the original `From`,
      `Cc` is the union of original `To`+`Cc` minus the account's own
      primary email/aliases.
- [ ] On a thread of 3 messages, reply to the latest. → `References`
      header includes prior `Message-ID`s in order, terminated by the
      message you replied to.
- [ ] Reply to a message **without** a `Message-ID`. → composer still
      opens; `In-Reply-To` is empty; `References` falls back gracefully
      (no panic, no garbage value).
- [ ] Press `F`. → composer opens as a forward: `To` is empty,
      `Subject` is `Fwd: <original>` (collapsing existing `Fwd:` if
      present), body shows the quoted forward. Cached attachments on
      the original are pre-populated in the attachments panel.
- [ ] Forward a message with cached attachments. → attachments fetched
      via `attachment.fetch_for_forward`, materialised into temp
      files, present in the attachments panel before the user sends.
- [ ] Forward while offline (kill the IMAP connection: stop daemon's
      network, or block its outbound TCP). → for any attachment whose
      bytes are not in the local DB, an error toast lists the failed
      filenames (`Could not carry forward: <names>`); the composer
      still opens with the rest intact. Sending such a forward is
      blocked until either the missing attachments are fetched or
      manually removed.

**Known limitations**

- Subject normalisation only collapses an exact `Re: ` / `Fwd: `
  prefix. Variants like `Aw:`, `Re[2]:`, or localised prefixes are
  **not** stripped — they will compound. See
  `mail::reply::tests::*` for the exact contract.
- Reply-all dedup is against the account's primary email; alias
  arrays in the future may need extending.

## Slice 8 — Drafts list, reopen, save, delete

- [ ] Switch to the Drafts folder for the current account (e.g.
      `:goto Drafts` or navigate with the folder pane). → the messages
      list shows the Drafts pane (newest first by `updated_at`).
      Drafts created in Slice 6/7 above appear here.
- [ ] Press Enter on a draft row. → composer opens with body, headers,
      To/Cc/Bcc, and attachments restored. Status `Compose (resumed)`.
- [ ] Edit the body. Press `Ctrl-S`. → status `Draft saved <uuid>`,
      composer remains open.
- [ ] Edit the body again. Run `:w`. → same as `Ctrl-S` — saves the
      draft.
- [ ] With **dirty** changes, press `Esc`. → confirm prompt `Discard
      unsaved compose? y/n`. `n` keeps you in the composer; `y`
      exits without saving (status `Composer discarded`).
- [ ] With dirty changes, save, then press `Esc`. → composer closes
      cleanly (no prompt) because nothing is dirty.
- [ ] In the Drafts pane, select a draft and press `d`. → status
      `Delete draft? y/n`. Press `n` → `Delete cancelled`. Press `d`
      then `y` → draft disappears optimistically; if the daemon
      refuses, the list re-fetches and the row reappears.
- [ ] Send a draft (`Ctrl-X`). After SMTP accepts, the row no longer
      appears in the Drafts pane (daemon removes the draft post-send).
- [ ] In the composer, press `:`. → opens the command bar (only
      outside the body field). Inside the body, `:` types a literal
      colon.

**Known limitations**

- Per Slice-8 worker decision, the Drafts-pane delete bind is
  lowercase `d` (consistent with the messages-list `d`-then-`y`
  flow). Capital `D` is not bound.
- There is no timed autosave: drafts persist on `Ctrl-S`, `:w`, send,
  and the explicit Esc/discard prompt only.

## Slice 9 — Attachment preview scroll/select/copy

- [ ] On a message with a text attachment (e.g. `.txt`, `.log`,
      `.md`), focus the attachments pane (`a` toggles attachments
      focus; or `Tab` until it's active).
- [ ] Press Enter on the attachment. → preview pane fills in. Status
      `Preview: j/k scroll  v select  y copy  Esc cancel`. The
      preview enters focused mode (keys drive the viewport).
- [ ] `j` / `↓` scroll one line down. `k` / `↑` scroll up. Bounds:
      `k` past the top stays at line 0; `j` past the bottom stays at
      the last line.
- [ ] `Page Down` / `Ctrl-D` scroll a half-page down; `Page Up` /
      `Ctrl-U` scroll a half-page up.
- [ ] `g` jumps to the top of the preview. `G` jumps to the bottom
      (last line aligned with the viewport).
- [ ] `v` enters visual line mode (status changes; current line
      highlighted as the anchor). Press `j` repeatedly → selection
      extends downward; selected lines stay highlighted. `Esc` clears
      the selection (status `Preview selection cleared`); a second
      `Esc` defocuses the preview entirely (status `Attachments`).
- [ ] With a multi-line selection active, press `y`. → status `<n>
      line(s) copied`; paste into another app (Wayland: `wl-paste`;
      X11: `xclip -o -selection clipboard`) and verify the lines match.
- [ ] `y` with no selection → status `No preview selection`.
- [ ] `y` with an empty selection (whitespace only) → status
      `Selection is empty`.
- [ ] If the system has no clipboard backend (CI sandbox / Wayland
      without `wl-clipboard`), `y` surfaces a red error like
      `Clipboard error: <reason>`.

**Known limitations**

- Preview supports text only. Binary attachments fall through to the
  default `No inline preview` message; `y` is meaningless there.
- The viewport height for keyboard preview-scroll is fixed at 6
  lines (`PREVIEW_KEY_VIEWPORT_LINES`); resizing the terminal does
  not rebound `Ctrl-D` / `Ctrl-U` step size.

## Cross-cutting

- [ ] `cargo fmt --check` — clean.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo test` — green.
- [ ] `cargo deny check` — clean (advisories, bans, licenses, sources
      all green; required CI gate per H2 / `.github/workflows/ci.yml`).
- [ ] `bash bench/run.sh` (per H1). Compare against `bench/baseline.md`
      from `4dff6fe`:
  - cold start ≈ 24 ms (target < 50 ms);
  - daemon idle RSS ≈ 9.7 MB (target < 30 MB);
  - IPC ping p50 ≈ 14 µs (target < 1 ms);
  - DB read p50 ≈ 37 µs (target < 5 ms);
  - email parse ≈ 275k msgs/sec (target > 5k);
  - `postbloxd` stripped ≈ 5.84 MiB (⚠ over the 5 MB target,
    within the 10 MB hard limit — flag here, not blocking).
- [ ] No new `.factory/`/scratch artifacts in `git status` after the
      run.

## Defects log

Capture findings in this section while running. For each defect log
file, repro steps, expected, actual. Do not fix in this slice — open
follow-up tickets and link them here.

| # | File / Area | Repro | Expected | Actual | Follow-up |
|---|---|---|---|---|---|
| 1 |   |   |   |   |   |
| 2 |   |   |   |   |   |
