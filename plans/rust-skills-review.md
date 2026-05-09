# Rust-Skills Repo Review — Findings (R6 → R7+)

Read-only audit of the postblox crate against the 179-rule `rust-skills` skill
library at `.factory/skills/rust-skills/`. **No code changed.** All findings
are sourced from six parallel cluster reviewers. This document is the input
queue for future R7+ phases.

- **Repo head at review time:** branch `dev` @ `c922354` (R6 closure).
- **Source LoC reviewed:** ~29,148 across 47 files in `src/`.
- **Reviewers:** 6 clusters (Ownership, Errors, Memory/Perf, API/Types, Async/Codegen, Hygiene).
- **Calibration:** All findings observe CLAUDE.md anti-bloat rules. No
  recommendation introduces a new abstraction unless the third concrete case
  already exists or the rule's correctness benefit is established.

> Severity legend (codebase-relative):
> **CRITICAL** = correctness / panic-on-input / blown perf target ·
> **HIGH** = clear-win idiomatic improvement ·
> **MEDIUM** = polish that future-proofs the surface ·
> **LOW** = taste / drive-by hygiene.

---

## 0. Executive summary

Postblox is in **healthy shape** against the rust-skills checklist. The one
shared theme across every cluster: documentation and crate-level lint
attributes lag the structural and runtime discipline of the codebase.

| Cluster | CRIT | HIGH | MED | LOW | Notes |
|---|---|---|---|---|---|
| A. Ownership / Borrowing | 0 | 0 | 2 | 11 | Clean. No `&Vec`, no `&String`, no `RefCell` misuse, no globals. |
| B. Error Handling | 0 | 4 | 7 | 6 | `db/` leaks raw `sqlx::Error`; `attachments.rs` smuggles errors via `sqlx::Error::Protocol`; one silent `let _ = upsert(...)` in dispatcher. |
| C. Memory / Performance | 3 | 6 | 7 | 5 | Bench harness lacks `black_box` (perf gate may be lying); per-frame TUI clone+format storm; per-event IPC `Vec<u8>` allocations. |
| D. API / Types | 2 | 4 | 6 | 5 | Stringly-typed dispatch (`op: String`), all six entity IDs are raw `Uuid`, wire-format enums lack `#[non_exhaustive]`. |
| E. Async / Codegen | 0 | 6 | 7 | 5 | TUI does `std::fs` + `std::process` from async; MCP `wait_for_approval` polls instead of subscribing; `accept_loop` shutdown via `abort()`. |
| F. Naming / Tests / Docs / Project / Lints | 1 | 4 | 8 | 6 | Zero `# Errors` / `# Panics` / `# Safety` doc sections; no crate-level `#![deny/warn]` attributes; `dispatcher.rs` (2.1 k lines) has no inline tests. |
| **Totals** | **6** | **24** | **37** | **38** | **105 findings** |

The three changes most likely to move shipped behavior:
1. **C1** — Add `black_box` to `postblox-bench.rs` so the >5,000 msgs/sec gate
   is not measuring DCE'd code.
2. **H4 (async)** — Replace `wait_for_approval` polling with the existing
   `Topic::McpApprovalDecided` subscription.
3. **H1 (perf)** — Decide explicitly whether `opt-level = "z"` is worth the
   parser/ping perf cost, or split a `release-perf` profile.

The single biggest documentation-debt item: every public fallible function
in the crate lacks a `# Errors` section (zero matches in `src/`).

---

## 1. Compliance highlights — what postblox already does right

These are recorded so future workers know which patterns to preserve when
they touch adjacent code.

### Architecture & runtime
- **Bounded channels everywhere.** `WRITER_MAILBOX = 128`, `MAX_CONNECTIONS = 64`,
  `MAX_SUBS_PER_CONN = 32`, hub broadcast capacity 256, `OUTPUT_MAILBOX = 128`,
  `SUBSCRIPTION_MAILBOX = 256`, `KEY_EVENT_CHANNEL_CAPACITY = 64`. Zero
  unbounded channels in `src/`.
- **No globals.** No `lazy_static!`, no `once_cell` globals, no hidden
  mutable statics. State threaded via `SqlitePool`, `Arc<Hub>`,
  `Arc<DaemonDispatcher>`.
- **Bounded resources.** DB pool max 8 / 3 s acquire; IPC frame max 4 MiB
  enforced on read and write (`ipc/wire.rs:14, 46-49`); `db::messages::list`
  uses `limit.clamp(1, 500)` (`src/db/messages.rs:96`).
- **IDLE workers** all use `CancellationToken` + `tokio::select!`
  (`src/sync/worker.rs`), tracked in `Mutex<HashMap<Uuid, JoinHandle>>`
  (`src/sync/manager.rs`).
- **`oneshot` per RPC** in `src/ipc/client.rs`. `RwLock<HashMap>` in `Hub`
  is never held across `await`.

### Error handling
- **`thiserror` per module, `anyhow` confined to bins.** Every module exposes
  a typed enum (`ConfigError`, `GoogleOAuthError`, `BridgeError`, `TuiError`,
  `MailboxError`, `CommandRunError`, `ThemeError`, `CommandParseError`,
  `SmtpError`, `SecretError`, `KeyringSecretError`, `SyncError`, `WireError`,
  `ClientError`, `ServerError`, `ImapError`, `EmbeddingError`, `MailError`,
  `DbError`). No giant `AppError`.
- **`#[from]` source chaining** is consistent across `config.rs`,
  `oauth/google.rs`, `sync/error.rs`, `secrets/mod.rs`, `ipc/wire.rs`.
- **`anyhow` strictly confined** to `main.rs`, `bin/postbloxd.rs`,
  `bin/postblox-mcp.rs`, `bin/postblox-bench.rs`. No imports anywhere else.
- **Worker error paths log structured context** before continuing
  (`src/sync/worker.rs:184, 196, 293, 305, 327, 383, 395, 457, 471`) — exemplary
  `err-context-chain` usage.

### Type safety
- **OAuth secret redaction** via manual `Debug` on `AccessToken`/`RefreshToken`
  (`src/oauth/google.rs`).
- **Typed `Topic` enum** at the Hub API; only the wire seam is stringly typed.
- **`Disposition` enum** in `mail/parser.rs` (Inline / Attachment / Other).
- **`GateDecision` enum** in `mcp/gates.rs` (Allow / Deny / Ask).

### Memory & perf hygiene
- `Vec::with_capacity` in `src/sync/reconciler.rs:107` and
  `src/mail/reply.rs:235-242`.
- `Hub` broadcasts `Arc<serde_json::Value>` so subscribers share payload
  allocation.
- `mail/threading.rs:60-69` `normalize_subject` uses `eq_ignore_ascii_case`
  on byte slices for prefix peeking.
- `db/messages.rs` and `db/search.rs` use `&'static str` for the column lists.
- Bench (`src/bin/postblox-bench.rs:69, 87`) has explicit warmup loops.

### Naming & structure
- Acronyms-as-words convention followed: `Mcp*`, `Smtp*`, `Imap*`, `Tls`,
  `Mime`, `Json`, `Http`, `Uuid`, `OpenAiProvider`, `GoogleOAuth*`.
- All `get_*` methods are fallible `Option`-returning lookups (correct usage).
- `mod.rs` per multi-file module; `src/bin/` for binaries; `pub(crate)` test
  helpers; thin `main.rs`.
- 43 modules already have `#[cfg(test)] mod tests` blocks.
- `Cargo.toml` metadata is complete: `description`, `license = "MIT OR Apache-2.0"`,
  `repository`, `homepage`, `documentation`, `readme`, `keywords`, `categories`,
  `include`.
- CI has fmt + clippy `-D warnings` + test + deny + audit + doc-warnings +
  coverage + MSRV jobs.

---

## 2. Critical findings (6) — fix in next perf/correctness phase

### CRIT-1 · Bench harness can be eliminated by the optimizer
- **Cluster:** C (perf-black-box-bench)
- **Files:** `src/bin/postblox-bench.rs:60-66, 78-84, 99-103`
- **Symptom:** All three benchmark modes assign work to `let _ = ...` and
  discard. With `lto = true, codegen-units = 1, opt-level = "z"`, the
  optimizer is free to delete code whose output is unused. The H1 perf
  baseline in `bench/baseline.md` may be measuring a heavily DCE'd loop.
- **Why it matters:** The "≥5,000 msgs/sec parser throughput" gate in
  CLAUDE.md is unenforceable until inputs and outputs are wrapped with
  `std::hint::black_box`.
- **Fix sketch:**
  ```rust
  let raw = std::hint::black_box(&corpus[i % corpus.len()]);
  let parsed = mail::parser::parse(raw).expect("parse");
  std::hint::black_box(parsed);
  ```
- **Effort:** S (≈5 lines).

### CRIT-2 · Per-frame TUI clone + format storm
- **Cluster:** C (anti-format-hot-path, mem-reuse-collections, mem-with-capacity)
- **Files:** `src/tui/render.rs:85, 107, 109, 113, 181-187, 218-219, 257-262, 307-311, 343-348, 416-419, 397, 433-485, 769-815, 840-852, 864`
- **Symptom:** Each TUI render calls `format!` ≥30 times for fixed-shape
  strings, clones every visible row's label/subject/snippet, materializes
  intermediate `Vec<Line>` via `.collect::<Vec<_>>()`, and joins per-account
  `String`s for the status bar. ratatui takes `Cow<'_, str>` everywhere via
  `Span::raw(impl Into<Cow<'_, str>>)`, so the clones aren't required.
- **Why it matters:** A typical render with 10 accounts × 30 visible threads
  × 30 visible messages allocates and frees on the order of 100+ short
  `String`s plus a `Vec<Line>` per keystroke. Pressures the allocator and
  competes with the IPC reader on the same Tokio runtime.
- **Fix sketch:**
  1. Replace `Span::raw(account.label.clone())` with
     `Span::raw(account.label.as_str())` — ratatui borrows.
  2. Pre-format `&'static str` arrays for fixed-shape role/state suffixes.
  3. `Paragraph::new(iter)` directly (it accepts iterators of `Line`).
- **Effort:** M (60-90 lines, low risk).

### CRIT-3 · Per-event IPC allocation churn
- **Cluster:** C (mem-reuse-collections, anti-format-hot-path)
- **Files:** `src/ipc/server.rs:269-276, 319-329`, `src/ipc/wire.rs:54, 69`
- **Symptom:** Every event flowing from `Hub` to a subscribed client allocates
  a fresh payload `Value` clone, an `Event` struct, a fresh `Vec<u8>` from
  `serde_json::to_vec`, plus `let mut buf = vec![0u8; len]` per inbound frame.
  `topic_name.into()` (`server.rs:277`) is `&'static str → String` per event.
- **Why it matters:** With `mail.new` + `mail.updated` + `sync.state` +
  `mcp.approval_requested` topics, an active sync produces dozens of
  events/sec; allocation alone can chew tens of microseconds against the
  IPC ping p50 < 1 ms target.
- **Fix sketch:**
  1. Make `Event::topic` a `&'static str` or `Cow<'static, str>`.
  2. Reuse a per-connection encode buffer; `serde_json::to_writer(&mut buf, &event)`.
  3. Reuse a per-connection read buffer; `buf.resize(len, 0)`.
- **Effort:** M (~30 lines per change, contained to `ipc/wire.rs` + `ipc/server.rs`).

### CRIT-4 · IPC dispatch is stringly-typed end-to-end
- **Cluster:** D (anti-stringly-typed, type-no-stringly)
- **Files:** `src/ipc/protocol.rs` `Request { op: String, .. }` →
  `src/ipc/server.rs::Dispatcher::dispatch(op: &str, args: Value)` → daemon
  open-coded match arms.
- **Symptom:** The op vocabulary is closed and known at compile time, but
  modeled as `String` end to end. Renames/typos invisible at compile time.
- **Why it matters:** Same risk class as MCP tool dispatch (`H4` below);
  both seams convert untyped strings into runtime errors.
- **Fix sketch:** Keep wire repr as a string; add an internal
  `enum Op { ... }` with `FromStr<Err = ParseOpError>` and have the
  dispatcher signature take typed `Op` (and a typed `Args` per op). Existing
  `text_enum!` macro is the natural pattern.
- **Effort:** M (mechanical; touches dispatcher).

### CRIT-5 · Wire-format enums missing `#[non_exhaustive]`
- **Cluster:** D (api-non-exhaustive)
- **Files:** `src/ipc/protocol.rs` (`Frame`, `RpcError`), `src/ipc/hub.rs`
  (`Topic`), `src/mcp/protocol.rs` (`Incoming`, `JsonRpcError`), `src/mcp/gates.rs`
  (`GateDecision`).
- **Symptom:** None of the wire-format / public-API enums carry
  `#[non_exhaustive]`. Adding a variant later is a SemVer break for any
  external matcher (downstream MCP clients, future cross-version daemon/TUI).
- **Fix sketch:** One-line attribute per enum. No behavior change.
- **Effort:** S.

### CRIT-6 · Public fallible API has zero `# Errors` / `# Panics` / `# Safety` sections
- **Cluster:** F (doc-errors-section, doc-panics-section, doc-safety-section)
- **Files:** `src/` (zero matches for `# Errors` / `# Panics` / `# Safety`).
- **Symptom:** Hundreds of `pub fn ... -> Result<T, E>` (every `db/*`,
  `oauth/*`, `mcp/*` tool, `ipc/*` codec). None document failure modes.
  `bounded_http_client` (`src/oauth/google.rs:313`) panics on TLS init failure
  with no `# Panics` doc; `mail/builder.rs:140` has a `// SAFETY:` line but
  no `# Safety` section on the surrounding `pub fn`.
- **Why it matters:** `cargo doc` output is unhelpful; future readers (and
  AI agents working on this crate) cannot infer error contracts without
  reading the body. CI already runs `cargo doc` with
  `RUSTDOCFLAGS="-D warnings"`, so adding `#![warn(missing_docs)]` would
  start failing immediately — staged adoption is required.
- **Fix sketch:** Phase 1: add `# Errors` to `db/*`, `oauth/*`, `ipc/*`,
  `secrets/*` public functions. Phase 2: enable `#![warn(missing_docs)]`
  globally. Phase 3: add `# Panics` for any remaining `unwrap`/`expect`
  call site that survived the err-cluster fixes.
- **Effort:** L (staged across two phases).

---

## 3. High-priority findings (24)

Ordered by cluster, with citations and quick fix sketches. Anything marked
`(R7+)` is a clear long-tail item; everything else is appropriate for an
R6.x patch phase.

### Error handling

- **B-H1 · `db/` leaks raw `sqlx::Error`.** `DbError` exists in
  `src/db/mod.rs:21-27` but is only used by `connect`. Every entity function
  returns `Result<_, sqlx::Error>` directly. Callers (`sync/error.rs:11`,
  dispatcher, attachments) re-map raw sqlx variants. Fix: expose `DbError`
  from each entity module or split typed variants per entity (`NotFound`,
  `DuplicateUid`, `ConstraintViolation`). Pairs with **B-M6** (`SyncError::Db`
  should wrap `DbError`).
- **B-H2 · `attachments.rs` smuggles non-DB errors through
  `sqlx::Error::Protocol` / `Io`.** `src/attachments.rs:42-45, 209-212, 230-232`.
  Define `AttachmentError` (`TooLarge { filename, limit }`, `BadPath`,
  `#[from] sqlx::Error`, `#[from] std::io::Error`) per `err-custom-type`.
- **B-H3 · Silent `let _ = db::folders::upsert(...)` in dispatcher.**
  `src/daemon/dispatcher.rs:1590-1601`. CLAUDE.md explicitly forbids silent
  `let _ =` on `Result`. Fix: `if let Err(e) = ... { tracing::warn!(...) }`
  or surface the first error.
- **B-H4 · Production constructor panics via `.expect()` for recoverable init.**
  `src/daemon/dispatcher.rs:120-122` calls `imap::default_auth().expect(...)`
  three times. Convert `DaemonDispatcher::new` to return
  `Result<Self, DispatcherInitError>`; thread through `bin/postbloxd.rs`
  with one `?`.

### Memory / Performance

- **C-H1 · `opt-level = "z"` vs hard perf gates (decision needed).**
  `Cargo.toml:69-74`. CLAUDE.md ships >5,000 msgs/sec parser, IPC ping
  p50 < 1 ms, DB read p50 < 5 ms. `opt-level = "z"` minimizes binary size at
  the cost of inlining and auto-vectorization. Two options: switch to
  `opt-level = 3` (re-measure binary size first), or keep `"z"` and add a
  `release-perf` profile inheriting from `release` with `opt-level = 3` for
  the bench/CI lane and re-baseline. Decision item, not a mechanical fix.
- **C-H2 · `parse()` allocates `attachments` vec without `with_capacity`.**
  `src/mail/parser.rs:67`. Bounded upper bound is `message.parts.len()`.
  Fix: `Vec::with_capacity(message.parts.len().min(8))`.
- **C-H3 · `format!()` per part inside parser hot loop.**
  `src/mail/parser.rs:114-121, 127-129, 223-227`. With multipart messages,
  N parts × M headers = N+M fixed-shape allocations per message. Use a
  thread-local scratch `String` + `write!`, or return `&'static str` for
  common content types (already have `mime_guess`).
- **C-H4 · `existing_uids` builds intermediate `Vec<&str>` for `.join(",")`.**
  `src/db/messages.rs:130`. Build query directly with `String::with_capacity`.
- **C-H5 · `db/messages.rs` and `db/search.rs` rebuild SELECT strings per call.**
  `src/db/messages.rs:43-50, 72-73, 80-83, 90-93, 105, 116-119`,
  `src/db/search.rs:48-58, 67-71`. Fixed-shape queries — promote the assembled
  query strings to `const` at module scope. Where placeholder count varies,
  use the **C-H4** builder pattern.
- **C-H6 · `prefix_each_line` triple-pass with intermediate `Vec<String>`.**
  `src/mail/reply.rs:303-310`. Single-pass `String::with_capacity` + push.

### Async / Codegen

- **E-H1 · Synchronous file IO in async path (forward attachments).**
  `src/tui/mod.rs:795-814` (`materialise_forward_attachment`) does
  `std::fs::create_dir_all` + `std::fs::File::create` + `file.write_all`
  inside the TUI runtime task. Replace with `tokio::fs::write` or
  `spawn_blocking`.
- **E-H2 · Synchronous file IO in async path (draft attachments).**
  `src/tui/mod.rs:819-836` — same pattern, called from
  `composer_draft_from_summary` (`src/tui/mod.rs:845`) which is awaited at
  `:2182`.
- **E-H3 · Sequential awaits where `try_join!` parallelizes.**
  `src/daemon/dispatcher.rs:1242-1283` (`op_attachment_fetch_for_forward`):
  three independent DB lookups in series. Folder/account both depend on
  `message`, but folder and account are independent. Use
  `tokio::try_join!(folders::get, accounts::get)` after the messages call
  resolves. Saves one DB round-trip on the cold path.
- **E-H4 · MCP approval polling instead of broadcast subscription.**
  `src/mcp/server.rs:315-335` (`wait_for_approval`) busy-polls every
  ~250 ms. `Topic::McpApprovalDecided` is already published
  (`src/daemon/dispatcher.rs:688`, `src/ipc/hub.rs:138`) and the bridge
  subscribes (`src/mcp/server.rs:555`). Replace the loop with a subscription
  filter or oneshot keyed by approval id, plus existing timeout via
  `tokio::select!`. Biggest behavioural async win in the audit.
- **E-H5 · `xdg-open` spawn uses `std::process::Command`.**
  `src/tui/mod.rs:1931-1945` (`open_attachment_with_xdg`) calls
  `Command::new("xdg-open").arg(...).status()` synchronously inside an
  async event loop. Use `tokio::process::Command::status().await`.
- **E-H6 · `ServerHandle::shutdown` aborts the accept loop.**
  `src/ipc/server.rs:67-74`. `accept_loop` (`:114-`) has no shutdown signal;
  `abort()` cancels mid-accept and orphan per-connection writers as
  detached `JoinHandle`s. Pass a `CancellationToken` (or oneshot) into
  `accept_loop` and use `tokio::select!`. Track per-conn tasks in a
  `JoinSet` so shutdown drains in-flight handshakes.

### API / Type safety

- **D-H1 · All entity IDs are raw `Uuid`.** `src/models.rs` and `src/db/*`
  share six confusable kinds: `AccountId`, `FolderId`, `ThreadId`,
  `MessageId`, `DraftId`, `AttachmentId`. Cross-module signatures
  `db::messages::get(pool, id)` accept any UUID; arg-swap is compile-clean.
  Fix: `#[repr(transparent)] pub struct MessageId(Uuid);` per id, with
  `Display`, `FromStr`, `Serialize`, `Deserialize`, `From<Uuid>`. **Highest
  type-safety leverage in the crate.**
- **D-H2 · Typed JSON columns are `serde_json::Value`.** `src/models.rs`
  `MessageRow.to_addrs/cc_addrs/bcc_addrs` carry `Vec<EmailAddress>`-shape
  data; `flags` carries enum-set data; `raw_headers` carries header pairs.
  Stored as opaque JSON. Parse-don't-validate at the DB boundary:
  deserialize directly into typed Rust at `db::messages::row_to_message`.
- **D-H3 · `Topic::parse(&str) -> Option<Self>` rolls its own.**
  `src/ipc/hub.rs`. Implement `FromStr<Err = ParseTopicError>` + `Display`
  for free serde integration and a typed parse error. Same recommendation
  applies to the `text_enum!` macro's `FromStr<Err = String>` — switch to a
  typed `ParseEnumError`.
- **D-H4 · MCP tool names are `&'static str` with runtime arg validation.**
  `src/mcp/tools.rs`. 12 tools by name; argument schema validated at runtime.
  Internal `enum McpTool { Search, ListAccounts, … }` mapped at the
  JSON-RPC boundary catches rename-misses at compile time.

### Documentation / Lints

- **F-H1 · `lib.rs` has no crate-level `//!` documentation.** `src/lib.rs` is
  a bare `pub mod X;` list. This is the docs.rs landing page. CLAUDE.md ships
  the canonical "what is postblox" prose — at minimum that paragraph
  belongs at the crate root.
- **F-H2 · No lint attributes on crate roots.** `src/lib.rs`, `src/main.rs`,
  three `bin/*.rs`. Crate relies entirely on CI's `-D warnings`. Local
  `cargo clippy` won't fail. Recommended baseline:
  ```rust
  #![deny(clippy::correctness)]
  #![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
  #![warn(missing_docs)]
  #![warn(clippy::undocumented_unsafe_blocks)]
  #![deny(unsafe_op_in_unsafe_fn)]
  ```
  Note: `missing_docs` will fail until **CRIT-6** is staged.
- **F-H3 · `dispatcher.rs` (2,101 lines) has no `#[cfg(test)] mod tests`.**
  Largest file in the crate. All coverage is integration-only via
  `tests/ipc_integration.rs`. Helpers like op-string→handler dispatch and
  OAuth refresh wiring (`src/daemon/dispatcher.rs:1237, 1387, 1554`) have
  no unit tests. Add a tests module with handler-level cases before any
  refactor.
- **F-H4 · No `proptest` and no `criterion`.** `Cargo.toml` has neither.
  `mail/parser.rs`, `mail/threading.rs`, `mail/reply_extract.rs`,
  `mail/builder.rs` are explicitly framework-free per CLAUDE.md and the
  highest-ROI targets for property-based testing (MIME boundaries, header
  parsing, quote pruning). `criterion` would replace the homemade
  `postblox-bench.rs` with statistical-significance reporting and
  `cargo bench` integration.

---

## 4. Medium-priority findings (37)

Grouped by cluster. Cite by ID where the cluster reviewer assigned one;
some have file:line only.

### A. Ownership / Borrowing
- **A-M1 · Attachment-byte cloning in dispatcher.** `daemon/dispatcher.rs:1347-1358`.
- **A-M2 · Attachment-tuple flattening.** `daemon/dispatcher.rs:1171-1183`.
- **A-PR1 (drive-by) · Elidable `<'a>` lifetime.** `src/mail/parser.rs:188`.
- **A-PR2 (drive-by) · `draft.subject.as_deref().unwrap_or("")` saves one
  alloc per send.** `src/daemon/dispatcher.rs:1417`.

### B. Error handling
- **B-M1 · Capital-cased thiserror messages.** `src/tui/mod.rs:1552-1556`
  ("No account selected", etc.). Lowercase per `err-lowercase-msg`.
- **B-M2 · Missing `BUG:` prefix on infallible `expect`.**
  `src/oauth/google.rs:308-313` `bounded_http_client`.
- **B-M3 · `.expect("folder we just updated")` after upsert.**
  `src/db/folders.rs:50`. Either return `DbError::NotFound` or prefix with
  `BUG:`.
- **B-M4 · `.expect("checked above")` re-extracts `composer_draft_id`.**
  `src/tui/mod.rs:1893`. Hold value once.
- **B-M5 · `serde_json::Error` manufactured to wrap empty frames.**
  `src/ipc/wire.rs:40-46`. Add `WireError::EmptyPayload` variant.
- **B-M6 · `SyncError::Db(#[from] sqlx::Error)`.** `src/sync/error.rs:10-11`.
  Should wrap `DbError` once **B-H1** lands.
- **B-M7 · `db::test_pool` panics with bare strings.** `src/db/mod.rs:71, 75`.
  Test-only; add `BUG:` prefix.

### C. Memory / Performance
- **C-M1 · Repeated `RpcError::internal(format!("op_name: {e}"))` in
  dispatcher.** 15× across `src/daemon/dispatcher.rs:498, 502, 571, 588, …`.
  Error path; safe to leave (optional helper). Flag, do not chase perf.
- **C-M2 · Reconciler clones address lists from owned `parsed`.**
  `src/sync/reconciler.rs:255-256, 261`. Take owned `parsed: ParsedEmail` and
  `into_iter()` the address lists.
- **C-M3 · `chars().take(200).collect().replace(...)` two-pass.**
  `src/sync/reconciler.rs:236-239`. Single-pass with `String::with_capacity(200)`.
- **C-M4 · `normalize_subject` allocates per thread per message.**
  `src/mail/threading.rs:67, 33-50`. Pre-normalize once when building
  `ThreadRef`, or return `Cow<'_, str>`.
- **C-M5 · Speculative `kind` field on `OutFrame`.**
  `src/ipc/server.rs:147-152` carries `#[allow(dead_code)] // future flow control`.
  Delete per CLAUDE.md "no abstractions before the third use".
- **C-M6 · `Message` struct size guard absent.** `src/models.rs:158-181`.
  Add `const _: () = assert!(std::mem::size_of::<Message>() <= 320);`. Audit
  whether list views need bodies (consider `MessageMeta` for list, `Message`
  for detail).

### D. API / Type safety
- **D-M1 · `Frame` is `#[serde(untagged)]` + not `#[non_exhaustive]`.**
  `src/ipc/protocol.rs`. Tag it (`#[serde(tag = "kind")]`) or at minimum mark
  non-exhaustive.
- **D-M2 · `EmbeddingProvider` trait has one impl.**
  `src/embeddings/mod.rs` + `openai.rs`. Tension between
  `anti-over-abstraction` ("third use") and CLAUDE.md "Mock at boundaries
  (traits)". Resolve by either adding the second backend in R7+ or
  documenting the mock-boundary justification in `embeddings/mod.rs`.
- **D-M3 · `EmbeddingProvider::embed` returns `Pin<Box<dyn Future>>`.**
  Inconsistent with `#[async_trait]` used by `Dispatcher`. Either AFIT
  (`async fn embed`) or `#[async_trait]`.
- **D-M4 · `text_enum!` produces `FromStr<Err = String>`.** Replace with a
  typed `ParseEnumError`.
- **D-M5 · `build_mime_full` takes 9 positional arguments.**
  `src/mail/builder.rs` (`#[allow(clippy::too_many_arguments)]`). NOT a
  builder yet — only one caller chain. Flag for monitoring; switch to a
  `MimeBuildOptions` struct (not fluent builder) if a third caller appears.
- **D-M6 · Stringly-typed `Dispatcher` trait signature.** Same root cause as
  CRIT-4; once typed `Op` exists, change `dispatch(req: Request)`.

### E. Async / Codegen
- **E-M1 · `oauth/google.rs` constructs fresh `reqwest::Client` per call
  site.** `GoogleOAuthHttpClient::new`. Build one in `DaemonServices::new`,
  share via `Arc<Client>`.
- **E-M2 · Secrets file write_lock spans two `await` points.**
  `src/secrets/file.rs:153-158, 171-176`. Intentional `tokio::sync::Mutex<()>`
  for write coalescing; rule allows it. Add a SAFETY comment so future
  readers don't try to "fix" it.
- **E-M3 · `embeddings/openai.rs` boxed-future cloning hygiene.** Confirm
  request body construction lives inside `Box::pin(async move { ... })` so
  early cancellation drops cleanly.
- **E-M4 · Mailbox sizes match CLAUDE.md but lack `const_assert`s.** Add
  `const _: () = assert!(WRITER_MAILBOX == 128);` for documentation +
  regression resistance.
- **E-M5 · Cold/error paths missing `#[cold]`.**
  `src/ipc/protocol.rs::RpcError::{bad_args,internal,new}`,
  `src/daemon/dispatcher.rs::unknown_op`. With `lto=fat`, the impact is
  small; do as part of the next dispatcher pass.
- **E-M6 · `probe_attachment` does sync `std::fs::metadata`.**
  `src/tui/app.rs:2940-2966` (called at `:2585`). Use
  `tokio::fs::metadata(path).await`.
- **E-M7 · `config.rs` reads file synchronously at startup.**
  `src/config.rs:129`. Acceptable per `async-tokio-fs` (sync IO before
  runtime starts). No action.

### F. Naming / Tests / Docs / Project / Lints
- **F-M1 · Acronym casing: `Xoauth2` vs `OAuth2Bearer`.**
  `src/imap/client.rs:135, 140, 370` vs `src/smtp/mod.rs:125`. Pick one.
- **F-M2 · Hand-rolled mocks in `tests/ipc_integration.rs`.** Could use
  `mockall` against `GoogleOAuth`/`SmtpSubmitter`/`ImapClient`/`SecretStore`
  trait boundaries. Not correctness; reduces boilerplate.
- **F-M3 · `mod default_path_tests`** instead of `mod tests`.
  `src/ipc/mod.rs:36`. Either rename or accept the pattern.
- **F-M4 · Bench is a binary, not `criterion`.**
  `src/bin/postblox-bench.rs` + `bench/run.sh` is reproducible but not
  statistically rigorous. Pairs with **F-H4**.
- **F-M5 · Missing `//!` on hot-path modules.** `mail/parser.rs`,
  `mail/threading.rs`, `mail/builder.rs`, `mail/reply_extract.rs`,
  `mail/mod.rs`, `mail/error.rs`, `embeddings/{mod,openai}.rs`,
  `oauth/{mod,google}.rs`, `auth.rs`, `attachments.rs`,
  `tui/{app,render,theme,command,ipc,mod}.rs`, `smtp/mod.rs`,
  `db/{accounts,folders,threads,messages,attachments,audit,drafts}.rs`,
  `lib.rs`, `main.rs`.
- **F-M6 · `clippy.toml` is one line (`msrv = "1.80"`).** Add
  `disallowed-methods`, `disallowed-types`, `cognitive-complexity-threshold`,
  `too-many-arguments-threshold`, `type-complexity-threshold` — the
  dispatcher would surface drift before reviews catch it.
- **F-M7 · `.rustfmt.toml` is two lines.** Recommended additions:
  `imports_granularity = "Crate"`, `group_imports = "StdExternalCrate"`,
  `wrap_comments = true`, `comment_width = 100`,
  `use_field_init_shorthand = true`. Nightly-rustfmt only — gate via a
  shadow `rustfmt.nightly.toml`.
- **F-M8 · `lib.rs` lacks public re-exports.** Bare `pub mod X` list. With
  16 top-level modules, downstream callers reach into
  `postblox::db::accounts::get_by_email` instead of
  `postblox::get_account_by_email`. Cost-benefit borderline; flag for
  R7+ discussion.

---

## 5. Low-priority / drive-by findings (38)

These are best fixed during the next pass that touches the file. None are
gating.

### A. Ownership / Borrowing
- 11 LOW items: cosmetic `.clone().unwrap_or_default()` patterns; a few
  structurally-required clones noted as non-issues for completeness.
  All are documented in the cluster A reviewer report; see findings A-L*
  in the per-cluster archive.

### B. Error handling
- **B-L1 · `imap/mod.rs:85, 105`, `imap/client.rs:322, 345`** —
  `let _ = session.logout().await`. Add `// best-effort logout` comments.
- **B-L2 · `ipc/server.rs:71, 73, 83`** — `let _ = handle.await` /
  `remove_file`. Drop impl already has the canonical comment; mirror it on
  the explicit `shutdown` path.
- **B-L3 · `sync/reconciler.rs:174`** — `let _ = db::messages::delete(...)`
  rollback. Add `// best-effort rollback` comment.
- **B-L4 · `imap/client.rs:51`** — already documented ✓.
- **B-L5 · `mcp/server.rs:181`** — `let _ = out_tx.send(...)`. Comment.
- **B-L6 · `daemon/dispatcher.rs:900`** — already documented ✓.

### C. Memory / Performance
- **C-L1 · `Event::data: serde_json::Value` clone-per-subscriber.**
  `src/ipc/server.rs:271`. Change to `Arc<serde_json::Value>` so subscribers
  share the payload allocation. Pairs with CRIT-3.
- **C-L2 · `extract_addresses` collects then takes first.**
  `src/mail/parser.rs:60, 185-189`. Either inline or return `impl Iterator`.
- **C-L3 · `header_value_to_json`** allocates `Vec<Value>` for `TextList`
  / `Address`. `src/mail/parser.rs:212-220`. Cosmetic.
- **C-L4 · `attach_input.clone()` on composer render.**
  `src/tui/render.rs:649, 670-685`. Subset of CRIT-2.
- **C-L5 · `assign_thread` linear scan.** `src/mail/threading.rs:21-30`.
  Looks O(N²) but constants tiny. Per `anti-premature-optimize`, do not
  optimize until profiling shows it. Bench when threading is next touched.

### D. API / Type safety
- **D-L1 · `SecretStore` trait not sealed (R7+).** Postblox is
  single-crate; sealing is irrelevant unless the crate is split.
- **D-L2 · `#[must_use]` on `Result`-returning fns** is redundant with
  default `unused_must_use`. Skip.
- **D-L3 · `Default` impls** — spot-checked, nothing missing.
- **D-L4 · `From`/`Into` direction** — clean.
- **D-L5 · `#[serde(skip_serializing_if = "Option::is_none")]`** — wire
  structs in `protocol.rs` serialize `null` for absent options. Trivial
  cleanup pass.

### E. Async / Codegen
- **E-L1 · Release profile fully tuned.** Confirmed against
  `opt-lto-release`, `opt-codegen-units`, `opt-pgo-profile`, `opt-target-cpu`.
  PGO correctly deferred. `target-cpu=native` correctly absent (binary is
  distributed).
- **E-L2 · `opt-inline-small`/`opt-inline-always-rare` annotations not
  used.** With `lto=true, codegen-units=1`, cross-crate inlining is
  unrestricted; these are mostly cosmetic. Skip unless profile evidence.
- **E-L3 · `accept_loop` error fall-through lacks `#[cold]`.**
  `src/ipc/server.rs:114-122`. Cosmetic.
- **E-L4 · No SIMD/portable-simd opportunities.** Correctly absent.
- **E-L5 · Bounds-checked indexing in bench harness.**
  `src/bin/postblox-bench.rs` `samples_us[n / 2]`. Off-hot-path; acceptable.

### F. Naming / Tests / Docs / Project / Lints
- **F-L1 · `find_by_msgid_header` vs `get_by_*` siblings.**
  `src/db/messages.rs:118`. Pick one verb per crate.
- **F-L2 · Missing `tests/mail_integration.rs`.** `mail/` is the
  framework-free crown jewel and undertested at the integration boundary.
- **F-L3 · No `#![warn(missing_docs)]`.** Gated by CRIT-6 staging.
- **F-L4 · No `#![deny(unsafe_op_in_unsafe_fn)]`.** Cheap; pair with the
  F-H2 attribute pass.
- **F-L5 · `db/*` modules lack `//!` headers.** Subset of F-M5.
- **F-L6 · No doctests in the crate.** Any `///` fenced code block on
  `mail/` or `embeddings/` traits would double as a living example and
  CI-enforced contract.

---

## 6. Suggested phasing for R7+

Strictly an input queue. Each item references a finding ID above; these are
cohesive bundles, not individual PRs.

### Phase R7-A — Correctness & error contracts (one branch)
1. **B-H1 + B-H2 + B-M6** — promote `DbError`; introduce `AttachmentError`;
   re-wrap `SyncError::Db`. One coordinated change touching `db/*`,
   `attachments.rs`, `sync/error.rs`.
2. **B-H3** — replace silent `let _ = upsert(...)` with a logged warning.
3. **B-H4** — convert `DaemonDispatcher::new` to `Result`.
4. **B-M1** — lowercase the three TUI error messages.
5. **B-M2 / B-M3 / B-M4** — `BUG:` prefixes, hold the value once.
6. **B-M5** — `WireError::EmptyPayload` variant.
7. **B-L1 / B-L2 / B-L3 / B-L5** — best-effort comments on `let _ =` sites.

### Phase R7-B — Async correctness pass
1. **E-H4** — replace MCP polling with broadcast subscription. Biggest
   behavioural win.
2. **E-H1 / E-H2 / E-H5 / E-M6** — async-fs / async-process violations in
   TUI.
3. **E-H3** — `try_join!` in `op_attachment_fetch_for_forward`.
4. **E-H6** — cooperative shutdown of `accept_loop` (CancellationToken +
   `JoinSet`).
5. **E-M2 comment** — annotate the secrets file write-lock as intentional.

### Phase R7-C — Type safety overhaul
1. **CRIT-5 + D-M1** — `#[non_exhaustive]` pass on wire-format enums; tag
   the `Frame` enum.
2. **D-H1** — typed entity IDs (`AccountId`, `FolderId`, `ThreadId`,
   `MessageId`, `DraftId`, `AttachmentId`). `#[repr(transparent)]` newtypes
   with `Display`, `FromStr`, serde, `From<Uuid>`. Mechanical, high-leverage.
3. **D-H2** — typed JSON columns at the DB boundary
   (`Vec<EmailAddress>`, `Vec<Flag>`, header pairs).
4. **CRIT-4 + D-M6 + D-H4 + D-H3 + D-M4** — typed `Op` for IPC dispatch
   and MCP tool dispatch; typed `ParseEnumError` for `text_enum!` and
   `Topic::FromStr`.

### Phase R7-D — Perf gate hardening
1. **CRIT-1** — `black_box` in bench harness.
2. **C-H1** — release-profile decision (decide before re-baselining).
3. **CRIT-2** — TUI render clone+format storm.
4. **CRIT-3 + C-L1** — IPC per-event allocation churn.
5. **C-H2 / C-H3 / C-H4 / C-H5 / C-H6** — parser + DB hot paths.
6. **C-M2 / C-M3 / C-M4** — reconciler & threading cleanups.
7. **C-M6** — `Message` size guard + list-vs-detail audit.
8. **C-M5** — delete speculative `OutFrame::kind`.
9. **F-M4 + F-H4** — port `postblox-bench.rs` to `criterion` (single PR).

### Phase R7-E — Documentation & lint hardening
1. **F-H1** — crate-level `//!` on `lib.rs`.
2. **F-M5 / F-L5** — module `//!` headers across `mail/`, `embeddings/`,
   `oauth/`, `auth.rs`, `attachments.rs`, `tui/*`, `smtp/`, `db/*`.
3. **CRIT-6 (Phase 1)** — `# Errors` on every public `Result`-returning
   function in `db/`, `oauth/`, `ipc/`, `secrets/`, `mcp/tools.rs`.
4. **F-H2 + F-L4** — crate-level `#![deny/warn]` attributes on `lib.rs`,
   `main.rs`, `bin/*.rs`. `unsafe_op_in_unsafe_fn` deny.
5. **CRIT-6 (Phase 2)** — `#![warn(missing_docs)]` once Phase 1 lands.
6. **F-M6 + F-M7** — tune `clippy.toml` and `.rustfmt.toml`.

### Phase R7-F — Test surface
1. **F-H3** — inline tests in `dispatcher.rs`.
2. **F-H4** — add `proptest` for `mail/parser.rs`, `mail/threading.rs`,
   `mail/reply_extract.rs`. `criterion` for parser throughput, IPC ping,
   DB read.
3. **F-L2** — `tests/mail_integration.rs`.
4. **F-M2** — replace hand-rolled mocks with `mockall` against trait
   boundaries.

### Drive-by polish bin
- **A-PR1 + A-PR2** — elide a lifetime in `mail/parser.rs:188`; save one
  alloc in `dispatcher.rs:1417`.
- **D-M2 / D-M3** — embeddings trait audit (defer until R7 LLM-gateway
  plan crystallises).

---

## 7. Items deliberately not actioned

These were checked and are **correctly absent** or **deliberately deferred**
per CLAUDE.md and project policy. Future workers should not "fix" them
without re-reading the rationale here.

- **No `lazy_static!` / `once_cell` globals** — by design.
- **No `unbounded_channel` anywhere** — by design.
- **`SecretStore` not sealed** — single-crate; sealing is irrelevant.
- **`build_mime_full` is not a builder yet** — only one caller chain;
  CLAUDE.md "no abstractions before the third use" applies.
- **No prelude module** — not warranted at this size.
- **No workspace split** — explicit decision (CLAUDE.md decision log
  2026-03-10). Reconsider only when contributor count grows.
- **`target-cpu=native` is not set** — binary is distributed; this is
  correct.
- **PGO is not configured** — single-user binary, no representative profile
  workload yet.
- **`assign_thread` linear scan** — `anti-premature-optimize`.
- **Repeated `RpcError::internal(format!(...))` in dispatcher** — error
  path; not hot.

---

## 8. Per-cluster archives

The full reviewer transcripts (with every cluster's positives, themes, and
non-issues) live in the session output. If a reader needs the un-summarised
form for a specific finding, search for the finding ID (e.g. `B-H1`,
`CRIT-3`) in the originating cluster's report.

| Cluster | Reviewer | Finding count |
|---|---|---|
| A. Ownership / Borrowing | own-* + 4 anti rules | 13 |
| B. Error Handling | err-* + 4 anti rules | 17 |
| C. Memory / Performance | mem-* + perf-* + 3 anti rules | 18 |
| D. API / Type safety | api-* + type-* + 3 anti rules | 17 |
| E. Async / Codegen | async-* + opt-* + 1 anti rule | 18 |
| F. Naming / Tests / Docs / Proj / Lints | name-* + test-* + doc-* + proj-* + lint-* | 19 |
| **Total** | | **102 findings + 3 drive-by polish** |

— end of report —
