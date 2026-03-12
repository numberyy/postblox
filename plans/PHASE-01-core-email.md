# Phase 1 — Core Email

**Goal:** Native inboxes work end-to-end. Create an inbox, send email through
Stalwart, receive inbound email via webhook, parse MIME, assign threads.

**Duration:** 2-3 weeks

## Prerequisites

- Phase 0 complete
- Stalwart Mail Server running (Docker or bare metal)
- DNS configured for at least one test domain (or localhost testing)

## Deliverables

### API Endpoints

- `POST /api/v1/inboxes` — create inbox (triggers Stalwart account creation)
- `GET /api/v1/inboxes` — list inboxes
- `GET /api/v1/inboxes/:id` — get inbox
- `DELETE /api/v1/inboxes/:id` — delete inbox
- `POST /api/v1/inboxes/:id/messages` — send email
- `GET /api/v1/inboxes/:id/messages` — list messages (paginated)
- `GET /api/v1/inboxes/:id/messages/:msg_id` — get message
- `GET /api/v1/inboxes/:id/threads` — list threads
- `GET /api/v1/inboxes/:id/threads/:thread_id` — get thread with messages
- `POST /api/v1/webhooks` — register webhook
- `POST /internal/stalwart/inbound` — Stalwart webhook receiver

### Authentication

- API key auth via `Authorization: Bearer pb_...` header
- Organizations table, API keys table
- Auth middleware extracting org context

### Email Pipeline

- Inbound: Stalwart webhook → parse (mail-parser) → extract reply → assign thread → store → dispatch webhooks
- Outbound: API call → build MIME (mail-builder) → submit to Stalwart SMTP → store sent copy
- Threading: Message-ID / In-Reply-To / References header chain, subject fallback

### Database Migrations

- `organizations` — id, name, created_at
- `api_keys` — id, org_id, key_hash, prefix, name, created_at
- `inboxes` — id, org_id, email, display_name, stalwart_account_id, created_at
- `messages` — id, inbox_id, thread_id, message_id (RFC 2822), in_reply_to, references, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, extracted_text, direction, created_at
- `threads` — id, inbox_id, subject, message_count, last_message_at
- `webhooks` — id, org_id, url, events, secret, active, created_at

### New Source Modules

```
src/api/mod.rs          — router assembly
src/api/auth.rs         — API key middleware
src/api/inboxes.rs      — inbox CRUD handlers
src/api/messages.rs     — message send/list/get handlers
src/api/threads.rs      — thread list/get handlers
src/api/webhooks.rs     — webhook CRUD handlers
src/core/mod.rs         — business logic
src/core/inbox.rs       — inbox creation/deletion logic
src/core/message.rs     — message processing pipeline
src/core/thread.rs      — thread assignment algorithm
src/db/mod.rs           — query modules
src/db/inboxes.rs       — inbox SQL
src/db/messages.rs      — message SQL
src/db/threads.rs       — thread SQL
src/db/webhooks.rs      — webhook SQL
src/db/api_keys.rs      — API key SQL
src/mail/mod.rs         — email parsing and building
src/mail/parser.rs      — mail-parser wrapper
src/mail/builder.rs     — mail-builder wrapper
src/mail/reply_extract.rs — reply/signature stripping
src/stalwart/mod.rs     — Stalwart HTTP API client
src/stalwart/client.rs  — account CRUD, domain management
src/events/mod.rs       — webhook dispatch
src/events/webhooks.rs  — HMAC signing, delivery with retry
```

### New Dependencies

- `mail-parser` — MIME parsing (Tier 2, behind trait)
- `mail-builder` — MIME building (Tier 2, behind trait)
- `reqwest` — Stalwart API + webhook delivery (Tier 2, behind trait)
- `argon2` or `blake3` — API key hashing
- `hmac` + `sha2` — webhook HMAC-SHA256 signing
- `uuid` — primary keys

## Test Strategy

### Unit Tests
- Config: all load paths, missing fields, invalid TOML
- Thread assignment: Message-ID chain, missing headers, subject-only fallback, orphan messages
- Reply extraction: plain text, HTML, nested quotes, signature stripping, edge cases (empty body, non-English)
- Auth: valid key, invalid key, missing header, expired key
- Webhook HMAC: correct signature, tampered payload

### Integration Tests (real Postgres)
- Inbox CRUD: create → list → get → delete
- Send + receive: send message → verify stored → verify thread created
- Webhook delivery: register → trigger event → verify POST received
- Full pipeline: create inbox → send → receive reply → thread assigned → webhook fired

### Fixtures
- `.eml` files: simple text, multipart, attachments, malformed, non-ASCII

## Checkpoint

Phase 1 is done when:

1. Create inbox via API → Stalwart account exists
2. Send email via API → recipient receives it
3. Receive inbound email → parsed, threaded, stored, webhook fired
4. `just check` passes
5. All integration tests pass against real Postgres + Stalwart
