# PostBox — Engineering Principles & Coding Guide

> This document defines how PostBox is built. Every contributor — human or AI — must
> follow these principles. If a PR violates any of these, it gets rejected.
>
> Read this before writing any code.

---

## 1. The One Rule

**Nothing gets built until something needs it.**

Not "might need it someday." Not "the spec says Phase 3." Not "it's only 50 lines."
If no user has hit the limitation, no test fails without it, and no current feature
depends on it — don't build it.

The spec documents are a map, not a construction plan. They prevent wrong turns.
They don't tell you to build every road today.

---

## 2. Anti-Bloat Rules

These are hard rules. Not guidelines. Not suggestions.

### 2.1 Every PR must be usable by itself

If a PR doesn't change what a user can do, it's premature abstraction. No PRs that
"add the trait that the next PR will implement." No empty module scaffolding.
No placeholder files. Every merge makes the product better for someone using it today.

### 2.2 No abstractions before the third use

Write the concrete implementation first. When you have three things that look
similar, then extract the trait or generic. Not before.

Wrong (day 1):
```rust
trait StorageBackend {
    async fn store(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn retrieve(&self, key: &str) -> Result<Vec<u8>>;
}

struct LocalStorage { ... }
struct S3Storage { ... }  // Nobody asked for this
```

Right (day 1):
```rust
async fn store_attachment(path: &Path, data: &[u8]) -> Result<()> {
    tokio::fs::write(path, data).await?;
    Ok(())
}
```

Right (when someone actually needs S3):
```rust
// NOW extract the trait, because we have two real implementations
trait StorageBackend { ... }
```

### 2.3 Delete code that isn't tested

If there's no test exercising a code path, that code path is a liability,
not a feature. Untested code is dead code with extra steps.

### 2.4 No feature flags that are never turned off

If something is always on, it's not a feature flag, it's dead code with an
if statement around it. Feature flags exist for deployment control, not for
"maybe someday" code paths.

### 2.5 Measure before optimizing

Add `tracing` spans to hot paths from day one. When something is slow, look
at the traces, don't guess. No speculative caching. No premature connection
pooling tuning. Profile first.

### 2.6 Dependencies earn their place

Before adding a crate, answer:
- Could I build this in under a day? If yes, consider building it.
- Does it pull in more than 10 transitive deps? That's a red flag.
- Is the crate actively maintained? Check last commit date.
- Is the license MIT/Apache? Run `cargo deny check` after adding.

Run `cargo tree` regularly. If you can't explain why a dependency exists,
remove it.

### 2.7 AI code gets compressed

After any AI-assisted coding session, do a compression pass:
- Can this function be half the lines?
- Does this struct need all these fields?
- Are these comments saying what the code already says?
- Is this error type actually used anywhere?
- Delete it and see if it compiles. If it compiles, it wasn't needed.

Rust's compiler is the ultimate bloat detector. If removing something doesn't
cause a compile error or test failure, it shouldn't exist.

---

## 3. Architecture Rules

### 3.1 The layer rule

```
api/     → Knows about HTTP. Extracts params, calls core, formats JSON.
core/    → Knows about business logic. NEVER imports axum, never knows about HTTP.
db/      → Knows about SQL. Returns domain types, not database rows.
mail/    → Knows about email. Wraps mail-parser/mail-builder.
```

A function in `core/` must be testable with zero network, zero HTTP, zero database.
Pass it the data it needs as arguments. Return the result. Let the caller deal
with I/O.

### 3.2 Error types are specific

```rust
// WRONG: One giant error enum for the whole app
enum AppError {
    Database(sqlx::Error),
    Mail(mail_parser::Error),
    Http(reqwest::Error),
    Io(std::io::Error),
    Config(toml::de::Error),
    // ... 20 more variants
}

// RIGHT: Each module has its own small error type
// core/inbox.rs
enum InboxError {
    NotFound(String),
    AlreadyExists(String),
    InvalidUsername(String),
}

// api/ converts these to HTTP responses at the boundary
impl IntoResponse for InboxError { ... }
```

### 3.3 No global state

All state lives in `AppState`, which is passed explicitly via Axum's
state extraction. No `lazy_static!`, no `once_cell` globals, no mutable
statics. If a function needs the database pool, it takes `&PgPool` as
a parameter.

### 3.4 Async only where it pays

Not everything needs to be async. Email parsing is CPU-bound and runs in
microseconds — make it a regular function. Only use `async` for actual I/O:
database queries, HTTP calls, IMAP connections, file reads.

```rust
// WRONG: async for no reason
async fn parse_subject(raw: &[u8]) -> Result<String> { ... }

// RIGHT: sync because there's no I/O
fn parse_subject(raw: &[u8]) -> Result<String> { ... }
```

---

## 4. Performance Targets

These aren't aspirational. They're the minimum bar for shipping.

### 4.1 Resource usage

| Metric | Target | Hard limit |
|--------|--------|------------|
| PostBox idle memory | < 10 MB | 20 MB |
| PostBox + Stalwart + Postgres idle | < 250 MB | 400 MB |
| Memory per linked IMAP connection | < 20 KB | 50 KB |
| Docker image size | < 20 MB | 30 MB |
| Binary size (stripped, release) | < 15 MB | 25 MB |

### 4.2 Latency (local Postgres, same machine)

| Operation | Target p50 | Target p99 | Hard limit p99 |
|-----------|-----------|-----------|----------------|
| GET /health | < 1 ms | < 5 ms | 10 ms |
| GET /inboxes (list) | < 3 ms | < 10 ms | 50 ms |
| GET /messages/:id | < 3 ms | < 10 ms | 50 ms |
| POST /send | < 50 ms | < 200 ms | 500 ms |
| POST /search (semantic) | < 20 ms | < 50 ms | 200 ms |
| Webhook dispatch | < 10 ms | < 50 ms | 200 ms |

### 4.3 Throughput

| Operation | Target (single core) |
|-----------|---------------------|
| Email parsing (mail-parser) | > 5,000 msgs/sec |
| API requests (simple reads) | > 10,000 req/sec |
| Concurrent IMAP syncs | 100+ accounts |
| WebSocket connections | 1,000+ |

### 4.4 Startup

| Metric | Target |
|--------|--------|
| Binary cold start to healthy | < 2 seconds |
| `docker compose up` to all healthy | < 30 seconds |
| First API response (including TLS) | < 100 ms |

### 4.5 Minimum host requirements

| Tier | Specs | Supports |
|------|-------|----------|
| Minimum | 1 core, 1 GB RAM, 10 GB disk | 5 inboxes, basic usage |
| Comfortable | 2 cores, 2 GB RAM, 20 GB disk | 50+ linked accounts, production |
| Recommended | 4 cores, 4 GB RAM, 40 GB disk | 100+ accounts, semantic search with Ollama |

---

## 5. Performance Principles

### 5.1 Don't cache what's cheap to recompute

mail-parser processes a 50KB email in microseconds. Don't cache parsed emails
in memory. Parse on demand from the database. Disk is cheap, RAM is not
(especially on VPSes).

### 5.2 Bound everything

Every queue, pool, buffer, and channel has a maximum size. Unbounded growth
is how processes get OOM-killed at 3am.

```rust
// WRONG: unbounded
let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

// RIGHT: bounded
let (tx, rx) = tokio::sync::mpsc::channel(1000);
```

Specific bounds:
- Postgres connection pool: max 10 (configurable)
- Concurrent IMAP syncs: max 50 (configurable)
- Webhook dispatch concurrency: max 20
- WebSocket connections: max 1000
- Request body size: max 10 MB
- Attachment size: max 25 MB

### 5.3 Connection pooling from day one

Use sqlx's built-in pool for Postgres. Don't create connections per-request.
The pool should be created once at startup and shared via AppState.

```rust
let pool = PgPoolOptions::new()
    .max_connections(10)
    .acquire_timeout(Duration::from_secs(3))
    .idle_timeout(Duration::from_secs(600))
    .connect(&database_url)
    .await?;
```

### 5.4 Zero-copy where it matters

mail-parser returns `Cow<str>` references into the original buffer. Don't
`.to_string()` everything immediately. Hold the reference as long as you
need it, only allocate when you must (e.g., storing in the database).

### 5.5 Async I/O, sync computation

Parsing, serialization, hashing — these are CPU-bound and fast. Do them
inline. Don't spawn tasks for microsecond operations. Only use `tokio::spawn`
for actual concurrent I/O work (IMAP sync tasks, webhook delivery batches).

### 5.6 Stream large responses, don't buffer

When returning large message lists or attachment downloads, use streaming
responses. Don't load everything into a Vec first.

```rust
// WRONG: buffer everything
let all_messages = db.fetch_all_messages(inbox_id).await?;
Json(all_messages)

// RIGHT: paginate at the database level
let page = db.fetch_messages(inbox_id, limit, offset).await?;
Json(page)
```

---

## 6. Code Style

### 6.1 Formatting and linting

- `cargo fmt` on every save. Non-negotiable.
- `cargo clippy -- -D warnings` passes. No suppressing warnings without a comment explaining why.
- Max line width: 100 characters (set in .rustfmt.toml).

### 6.2 Naming

- Types: `PascalCase` (Rust standard)
- Functions: `snake_case` (Rust standard)
- Constants: `SCREAMING_SNAKE_CASE`
- Modules: `snake_case`, one concept per module
- No abbreviations except universally understood ones (db, id, url, api)
- Name things for what they DO, not what they ARE:
  `send_message()` not `message_sender_execute()`

### 6.3 Comments

Comments explain WHY, not WHAT. If you need a comment to explain what
code does, the code is too complex — simplify it.

```rust
// WRONG: comment says what the code says
// Check if the inbox exists
if db.inbox_exists(inbox_id).await? { ... }

// RIGHT: comment explains a non-obvious decision
// We check existence before creating to return a clean 409 Conflict
// rather than letting the database UNIQUE constraint raise a less
// descriptive error.
if db.inbox_exists(inbox_id).await? {
    return Err(InboxError::AlreadyExists(inbox_id));
}
```

### 6.4 Error handling

- Use `thiserror` for library-style errors with structured variants.
- Use `anyhow` sparingly, only at the top-level binary boundary (main.rs).
- Every error variant must be actionable — if the caller can't do anything
  different based on the error variant, it shouldn't be a separate variant.
- Log errors at the point of handling, not at the point of creation.

### 6.5 Testing

- Unit tests live in the same file: `#[cfg(test)] mod tests { ... }`
- Integration tests live in `tests/` directory.
- Every public function in `core/` has at least one test.
- Use `pretty_assertions` for readable diffs.
- Tests are named `test_<thing>_<scenario>_<expected_result>`:
  `test_create_inbox_duplicate_username_returns_conflict`

---

## 7. Build & Release Rules

### 7.1 Release builds

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

This produces the smallest, fastest binary. `panic = "abort"` removes
unwinding code. `strip = true` removes debug symbols. `lto = true`
enables cross-crate optimization. `codegen-units = 1` maximizes
optimization at the cost of compile time (fine for releases).

### 7.2 Docker images

Multi-stage build. Final image is `scratch` or `alpine` with just the
binary and a CA certificate bundle. No shell, no package manager, no
runtime in the production image.

```dockerfile
FROM rust:1.75 AS builder
# ... build ...

FROM scratch
COPY --from=builder /app/postbox /postbox
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
ENTRYPOINT ["/postbox"]
```

### 7.3 CI must pass

No merging with broken CI. CI runs: `check`, `test`, `clippy`, `fmt`,
`deny` (license audit). If any fail, the PR is blocked.

---

## 8. What to Build, In What Order

This is the actual roadmap, stripped of ambition and grounded in the
anti-bloat rules above.

### Phase 0: Skeleton (1-2 days)

The binary starts. It connects to Postgres. It serves `/health`.
It loads config from `postbox.toml`.

Files that should exist after Phase 0:
```
src/main.rs          — Entry point, clap CLI
src/config.rs        — Load and validate TOML config
src/app.rs           — AppState, server startup
src/api/router.rs    — GET /health, GET /ready
src/api/error.rs     — Error → JSON response
src/db/mod.rs        — PgPool setup
migrations/001.sql   — organizations, api_keys tables
postbox.toml.example — Annotated example config
docker-compose.yml   — PostBox + Stalwart + Postgres
Dockerfile           — Multi-stage, scratch base
```

Files that should NOT exist after Phase 0:
```
src/mcp/             — No MCP server yet
src/sync/            — No linked inboxes yet
src/permissions/     — No permission engine yet
src/embeddings/      — No search yet
src/dashboard/       — No web UI yet
sdk/                 — No SDKs yet
```

### Phase 1: First email (2-3 weeks)

An agent creates an inbox, sends an email, receives a reply.
This is the minimum product that does something useful.

Additions:
```
src/api/auth.rs      — API key middleware
src/api/inboxes.rs   — Inbox CRUD
src/api/messages.rs  — Send + list messages
src/core/inbox.rs    — Inbox business logic
src/core/message.rs  — Message processing
src/mail/parser.rs   — Wrap mail-parser
src/mail/builder.rs  — Wrap mail-builder
src/mail/threading.rs — In-Reply-To/References
src/stalwart/client.rs — Stalwart REST API client
src/events/webhooks.rs — Basic webhook dispatch (message.received)
migrations/002.sql   — inboxes, threads, messages, attachments tables
```

Things explicitly NOT in Phase 1:
- Labels, drafts, domains, search
- Linked inboxes, Gmail relay
- Permission engine, approval workflows
- MCP server
- Web dashboard, TUI
- SDKs

### Phase 2: Make it useful (2-3 weeks)

Gmail relay mode (zero-config quick start). Labels. Drafts.
Outbound guard. Interactive CLI shell.

### Phase 3+: See the spec

Refer to the full spec and improvements docs. But only build what
Phase 2 users are actually asking for.

---

## 9. Decision Log

When a non-obvious decision is made, record it here. This prevents
re-litigating settled questions.

| Decision | Rationale | Date |
|----------|-----------|------|
| Rust + Axum | Performance, single binary, Stalwart crate ecosystem | 2026-03-10 |
| Postgres, not SQLite | pgvector, concurrent access, production-grade | 2026-03-10 |
| Stalwart as mail server | REST API, DKIM/SPF/DMARC, JMAP, actively maintained | 2026-03-10 |
| MIT license | Maximum adoption, no contributor friction | 2026-03-10 |
| No workspace (single crate) | Simpler for contributors, split later if needed | 2026-03-10 |
| Squash merges only | Clean git history | 2026-03-10 |
| cargo-deny with copyleft=deny | Protect future monetization from accidental GPL deps | 2026-03-10 |

---

## 10. When in Doubt

- Is this the simplest thing that could work? If not, simplify.
- Would I add this if the project had 0 users? If no, wait until it has users.
- Can I delete this and still have a working product? If yes, delete it.
- Does this PR make the binary bigger? Is that justified by what it enables?
- Am I building infrastructure or am I building features? Build features.
