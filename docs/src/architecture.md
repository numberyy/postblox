# Architecture

## Module map

```
src/
  main.rs           Entry point, Axum server, health endpoint
  config.rs         TOML config loading with env var fallback
  db.rs             PostgreSQL connection pool setup
  models.rs         Domain types (Organization, Inbox, Thread, Message, etc.)

  api/              HTTP handlers — auth, CRUD, inbound, search, briefing, rate limiting, bounce tracking
  core/             Business logic — slop classifier, rule engine
  db/               SQL queries via sqlx — one module per entity
  mail/             MIME parsing, reply extraction, threading, builder, guard
  stalwart/         Stalwart mail server HTTP + SMTP client
  events/           Webhook dispatch, WebSocket hub, notification dispatch
  hooks/            TOML-configured exec hooks — event hooks + before_send
  notifications.rs  Notification providers (ntfy, webhook, desktop)
  sync/             IMAP one-shot pull sync
  embeddings/       EmbeddingProvider trait, OpenAI-compatible HTTP client
  dashboard/        minijinja + htmx dashboard — 18 routes, 9 pages
  mcp/              postblox-mcp binary — stdio JSON-RPC, 44 tools
  tui/              postblox-tui binary — ratatui multi-panel TUI, 4 themes
```

## Layer rules

Dependencies flow downward only.

```
api/          HTTP handlers only. No business logic. Axum types stay here.
core/         Business logic. ZERO framework imports. Pure Rust.
db/           SQL queries via sqlx. Returns domain types, not raw rows.
mail/         Email parsing (mail-parser) and building (mail-builder).
stalwart/     Stalwart mail server HTTP client.
events/       Internal event bus, webhook dispatch, WebSocket hub.
permissions/  Permission engine, outbound guard, progressive trust.
hooks/        Exec hooks — shell commands triggered by events.
sync/         IMAP sync engine for linked inboxes.
embeddings/   Semantic search via pgvector.
```

Rules:
- `core/` must not import axum, sqlx, or any framework crate
- `api/` must not contain business logic
- `db/` must return domain types, never raw rows
- `mail/` wraps external crates behind internal types
- No module creates its own database connection — pool comes from AppState

## Inbound email flow

```
Stalwart SMTP
  │
  ▼
POST /internal/stalwart/inbound
  │
  ├── MIME parse (mail-parser)
  │     extract subject, body, from, to, headers
  │
  ├── Deduplication
  │     check Message-ID header against existing messages
  │
  ├── Threading
  │     match In-Reply-To / References headers to existing threads
  │     create new thread if no match
  │
  ├── Store
  │     insert message + thread into PostgreSQL
  │
  ├── Slop classification
  │     6-signal classifier scores for AI-generated content
  │
  ├── Embedding (if configured)
  │     generate vector embedding, store in pgvector
  │
  └── Event dispatch
        ├── Webhooks (HMAC-signed HTTP POST)
        ├── WebSocket (push to connected clients)
        ├── Hooks (exec shell commands)
        └── Notifications (ntfy, webhook, desktop)
```

## Outbound email flow

```
POST /api/v1/inboxes/{id}/messages
  │
  ├── Guard patterns
  │     regex scan subject + body, block if matched
  │
  ├── Permission check
  │     ├── shadow     → 403 rejected
  │     ├── approval   → 202, create approval record
  │     ├── auto_approve → evaluate rules (domain, keyword, slop, time)
  │     └── autonomous → proceed
  │
  ├── before_send hooks
  │     run each hook, fail-closed on error/block/timeout
  │
  ├── Store message in PostgreSQL
  │
  ├── MIME build
  │     construct email with proper headers, Message-ID, threading
  │
  └── Stalwart SMTP delivery
        send via lettre SMTP client to Stalwart
```

## Approval workflow

```
Agent sends message (inbox in "approval" mode)
  │
  ├── Message stored with direction=outbound
  ├── Approval record created (status=pending)
  ├── approval.requested event dispatched
  │
  ▼
Human reviews via API / TUI / Dashboard
  │
  ├── POST /approvals/{id}/approve
  │     ├── Message delivered via SMTP
  │     ├── Trust point recorded
  │     ├── If trust threshold reached → auto-upgrade to auto_approve
  │     └── Audit log entry
  │
  └── POST /approvals/{id}/reject
        ├── Message stays undelivered
        ├── Rejection recorded
        └── Audit log entry
```

## State management

All state lives in `AppState`, passed through Axum's `State` extractor:

```rust
pub struct AppState {
    pub pool: PgPool,                    // PostgreSQL connection pool
    pub stalwart: Option<StalwartClient>,// Mail server client
    pub webhook_client: reqwest::Client, // HTTP client for webhook dispatch
    pub inbound_token: Option<String>,   // Stalwart inbound auth
    pub guard_patterns: Vec<GuardPattern>,// Outbound content patterns
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub embedding_semaphore: Arc<Semaphore>,
    pub trust_auto_upgrade_threshold: i32,
    pub hooks: Arc<[HookConfig]>,
    pub ws_hub: Arc<WebSocketHub>,
    pub rate_limiter: Arc<RateLimiter>,
}
```

No global state. No `lazy_static!` or `once_cell`. Bounded channels for inter-task communication.

## Binaries

The project builds three binaries from a single crate:

| Binary | Entry point | Dependencies |
|--------|------------|--------------|
| `postblox` | `src/main.rs` | Full server — all modules |
| `postblox-mcp` | `src/mcp/main.rs` | HTTP client only — wraps REST API via reqwest |
| `postblox-tui` | `src/tui/main.rs` | HTTP + WebSocket client — ratatui UI |

The MCP server and TUI are independent clients of the REST API. They share no code with the server at runtime.
