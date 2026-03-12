# PostBox — Complete Technical Specification

> Open-source, self-hosted, performance-first email infrastructure for AI agents.
> The self-hosted alternative to AgentMail — with linked account sync that AgentMail doesn't have.

---

## 1. Design Philosophy

**Performance first.** Rust + Tokio. Zero-copy email parsing. Connection pooling everywhere. 3-5MB idle memory. Sub-millisecond API latency for cached operations. The kind of thing that runs on a $5 VPS alongside Stalwart and doesn't break a sweat.

**Minimal deep dependencies.** Every dependency is evaluated on two axes: (a) how long would it take to build ourselves, and (b) how much risk does the upstream introduce. We use dependencies where the domain is genuinely hard (MIME parsing, IMAP protocol, TLS), wrap them behind traits so we can swap later, and build everything else ourselves.

**Two inbox types.** Native inboxes (Stalwart-backed, full control) and Linked inboxes (Gmail/IMAP sync, agent acts on your real email). This is PostBox's killer feature over AgentMail.

**Two user tiers.** Non-technical users get a web dashboard with OAuth "Connect Gmail" flows and visual permission toggles. Technical users get a full REST API, TOML config, hook system, and MCP server.

---

## 2. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Client Layer                              │
│  Claude Code / Cursor / LangChain / CrewAI / Web Dashboard      │
└───────┬──────────────────┬──────────────────┬───────────────────┘
        │ REST (JSON)      │ WebSocket        │ MCP (stdio/SSE)
┌───────▼──────────────────▼──────────────────▼───────────────────┐
│                     PostBox Core (Rust)                           │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │  API Server  │  │  WS Hub      │  │  MCP Server            │ │
│  │  (Axum)      │  │  (tokio-ws)  │  │  (stdio + SSE)        │ │
│  └──────┬───────┘  └──────┬───────┘  └────────────┬───────────┘ │
│         │                 │                        │             │
│  ┌──────▼─────────────────▼────────────────────────▼───────────┐ │
│  │                    Unified Mailbox Engine                    │ │
│  │                                                             │ │
│  │  ┌─────────────────┐      ┌──────────────────────────┐     │ │
│  │  │  Native Inbox   │      │  Linked Inbox             │     │ │
│  │  │  (Stalwart)     │      │  (Gmail API / IMAP sync)  │     │ │
│  │  └────────┬────────┘      └──────────┬───────────────┘     │ │
│  │           │                          │                      │ │
│  │  ┌────────▼──────────────────────────▼──────────────────┐   │ │
│  │  │              Shared Services                          │   │ │
│  │  │  Threading │ Labels │ Search │ Attachments │ Drafts   │   │ │
│  │  └──────────────────────────────────────────────────────┘   │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │  Permission   │  │  Webhook     │  │  Sync Engine           │ │
│  │  Engine       │  │  Dispatcher  │  │  (per-account Tokio    │ │
│  │              │  │  (retries)   │  │   tasks + IMAP IDLE)   │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
│  │  Reply        │  │  Embedding   │  │  Audit Log             │ │
│  │  Extractor    │  │  Provider    │  │                        │ │
│  └──────────────┘  └──────────────┘  └────────────────────────┘ │
└──────────┬──────────────────────────────────────────────────────┘
           │
    ┌──────▼──────┐   ┌──────────────┐   ┌───────────────┐
    │ PostgreSQL  │   │  Stalwart    │   │ File Storage  │
    │ + pgvector  │   │  Mail Server │   │ (local / S3)  │
    └─────────────┘   └──────────────┘   └───────────────┘
```

### Why each piece exists

| Component | Justification |
|-----------|---------------|
| **Axum** | From the Tokio team. Minimal, fast, composable. No framework lock-in — it's just tower middleware and handlers. Stable API, actively maintained. |
| **Stalwart** | Handles SMTP in/out, SPF/DKIM/DMARC, IMAP serving, spam filtering. We call its REST API for account management. We do NOT fork it or deeply couple to its internals. |
| **PostgreSQL + pgvector** | Single database for everything: metadata, threads, permissions, audit log, embeddings. pgvector avoids a separate vector DB. |
| **Tokio** | Same async runtime as Stalwart. One runtime for everything: HTTP server, IMAP sync connections, background jobs, WebSockets. |
| **mail-parser** | Stalwart's own crate. Zero-copy, no external deps, 41 character sets, fuzz-tested, battle-tested with millions of real emails. Building our own MIME parser would take months and be worse. Low coupling risk — it's a pure parsing library with a stable API. |
| **mail-builder** | Stalwart's companion for constructing RFC 5322 messages. Same reasoning. |
| **async-imap** | For linked inbox sync. IMAP IDLE support for real-time notifications. Wrapping behind a trait so we can swap to Gmail API for Google accounts. |

---

## 3. Dependency Strategy

### Tier 1: Use and trust (stable, low-risk, domain is genuinely hard)

| Crate | Why | Lock-in risk |
|-------|-----|-------------|
| `tokio` | Async runtime. Industry standard. | Near zero — everything in Rust async uses it |
| `axum` | HTTP framework from tokio team | Low — it's just tower handlers, easy to swap |
| `sqlx` | Async Postgres with compile-time query checks | Low — generates standard SQL |
| `mail-parser` | Zero-copy MIME parsing. No deps itself. | Low — pure parser, stable API, we only call `parse()` |
| `mail-builder` | RFC 5322 message construction | Low — same as mail-parser |
| `rustls` | TLS without OpenSSL | Low — Mozilla-backed, widely used |

### Tier 2: Use behind traits (useful now, could replace later)

| Crate | Trait it sits behind | Replacement path |
|-------|---------------------|-----------------|
| `async-imap` | `trait ExternalMailProvider` | Gmail API client, custom JMAP client |
| `lettre` | `trait SmtpSender` | Direct SMTP via Stalwart's submission port |
| `reqwest` | `trait HttpClient` | hyper directly, or ureq for sync contexts |
| `oauth2` | `trait OAuthProvider` | Hand-rolled OAuth if the crate stalls |

### Tier 3: Build ourselves (core to our value, or simple enough)

| Component | Why build |
|-----------|----------|
| Permission engine | Core differentiator, must be deeply integrated |
| Reply extraction | Port of Talon's heuristics — regex + line analysis, not complex |
| Webhook dispatcher | Simple HTTP POST with retries + HMAC signing |
| WebSocket hub | Tokio broadcast channels, trivial |
| MCP server | JSON-RPC over stdio/SSE, small protocol surface |
| Sync engine | Orchestration logic is ours, IMAP transport is the crate |
| Audit log | Append-only Postgres inserts |
| Thread builder | RFC 2822 In-Reply-To/References header analysis |
| API auth | API key hashing + permission checks |

---

## 4. Inbox Types

### 4.1 Native Inbox

A real mailbox backed by Stalwart. Has its own email address (e.g., `agent@yourdomain.com`). Can send and receive email directly. Full control.

```
User creates inbox via API
  → PostBox calls Stalwart REST API to create account
  → PostBox stores inbox metadata in Postgres
  → Stalwart handles all SMTP/IMAP for that address
  → Inbound mail triggers Stalwart webhook → PostBox processes it
```

### 4.2 Linked Inbox

A proxy to an external email account (Gmail, Outlook, any IMAP). The agent reads and acts on your real email. Everything syncs back to the provider at configurable intervals.

```
User connects Gmail via OAuth (web dashboard)
  or provides IMAP credentials (API/config)
  → PostBox stores encrypted credentials
  → Sync engine starts a per-account Tokio task
  → Initial full sync: pull all messages into local Postgres cache
  → Ongoing: IMAP IDLE for real-time new message notifications
           + periodic full reconciliation (configurable: 1min — 1hr)
  → Agent operates on local cache (fast reads, instant search)
  → Write operations queue locally → push to provider on next sync
```

### 4.3 Sync State Machine

```
                    ┌─────────┐
          ┌────────►│  IDLE   │◄────────────┐
          │         └────┬────┘             │
          │              │ timer/IDLE        │
          │              │ notification      │
          │         ┌────▼────┐             │
          │         │ PULLING │             │
          │         └────┬────┘             │
          │              │                  │
          │         ┌────▼─────┐            │
          │         │RECONCILE │            │
          │         └────┬─────┘            │
          │              │                  │
          │    ┌─────────▼──────────┐       │
          │    │ conflicts?         │       │
          │    └──┬─────────────┬───┘       │
          │       │ no          │ yes       │
          │       │        ┌────▼─────┐     │
          │       │        │ CONFLICT │     │
          │       │        │ RESOLVE  │     │
          │       │        └────┬─────┘     │
          │       │             │           │
          │  ┌────▼─────────────▼───┐       │
          │  │    PUSH WRITES       │       │
          │  │  (queued local ops)  │       │
          │  └──────────┬───────────┘       │
          │             │                   │
          └─────────────┴───────────────────┘
```

### 4.4 Local operations between syncs

| Operation | Behavior | Sync action |
|-----------|----------|-------------|
| **Read message** | Instant from local cache | Mark as read on provider |
| **Label/star** | Applied locally | Push label via IMAP STORE or Gmail API |
| **Archive** | Move to archive locally | IMAP MOVE or Gmail modify |
| **Draft reply** | Stored in PostBox drafts | Optionally sync to provider's Drafts folder |
| **Send** | Queue locally → permission check → if approved, send via provider's SMTP/API | Sent message appears in provider's Sent folder |
| **Delete** | Soft-delete locally, queued | Trash or permanent delete on provider (configurable) |

### 4.5 Conflict resolution

- **Read status / labels:** Last-write-wins. Simple, predictable.
- **Message deleted by human while agent queued an action:** Agent action is discarded, logged in audit trail.
- **Human replied to thread while agent was drafting:** Agent draft is flagged as stale, not auto-sent. Notification to user.
- **New messages during sync:** Always additive — never lose mail.

---

## 5. Permission Engine

This is PostBox's major differentiator. AgentMail has no permission model — agents have full access to their inboxes. PostBox gives granular, auditable, configurable control over what agents can do.

### 5.1 Permission levels

```toml
# postbox.toml — per-inbox permission config

[inbox."support-agent@yourdomain.com".permissions]
read = true
label = true
archive = true
draft = true
delete = false              # agent cannot delete emails

[inbox."support-agent@yourdomain.com".permissions.send]
mode = "approval"           # "auto" | "approval" | "shadow" | "disabled"
```

### 5.2 Send modes

| Mode | Behavior | Use case |
|------|----------|----------|
| **auto** | Agent sends immediately, no human review | Trusted agents, internal comms |
| **approval** | Agent drafts → human gets notification → approve/reject | Customer-facing agents |
| **shadow** | Agent "sends" but nothing leaves. Draft is logged. Human reviews. | Onboarding new agents, testing |
| **disabled** | Agent cannot send at all | Read-only monitoring agents |

### 5.3 Approval flow

```
Agent calls POST /send
  → PostBox checks permission mode
  → If "approval":
      → Message stored as pending_send in Postgres
      → Notification dispatched:
          - WebSocket event to dashboard
          - Webhook to configured URL
          - (optional) push notification via ntfy/Pushover/email
      → Human reviews in dashboard or via notification link
      → Approve → PostBox sends the email
      → Reject → PostBox discards, logs reason
      → Modify → Human edits draft, then sends
      → Timeout → Configurable auto-action (default: discard after 24h)
```

### 5.4 Auto-approval rules

For the "approval" mode, you can define rules that auto-approve certain sends without human intervention:

```toml
[inbox."support-agent@yourdomain.com".auto_approve]
# Auto-approve replies to threads the agent is already in
replies_to_existing_threads = true

# Auto-approve sends to specific domains
domains = ["@yourcompany.com", "@partner.com"]

# Auto-approve if the email is under N characters
max_body_length = 500

# Rate limit: auto-approve up to N sends per hour
max_per_hour = 10
```

### 5.5 Progressive trust

A new concept — agents earn trust over time based on human feedback:

```
Shadow mode (new agent)
  → After 20 approved shadow actions → auto-promote to Approval mode
  → After 50 approved sends → auto-promote to Auto for replies
  → After 200 approved sends → auto-promote to full Auto

Trust score is per-inbox and resets if the agent triggers a rejection or complaint.
```

This is configurable and optional. Default is static permissions.

### 5.6 Audit log

Every action an agent takes is logged:

```json
{
  "timestamp": "2026-03-10T12:00:00Z",
  "inbox_id": "support-agent@yourdomain.com",
  "action": "send",
  "status": "pending_approval",
  "target": "customer@example.com",
  "subject": "Re: Billing question",
  "permission_mode": "approval",
  "auto_approve_matched": false,
  "trust_score": 42,
  "resolved_at": null,
  "resolved_by": null
}
```

Queryable via API: `GET /api/v1/audit?inbox_id=...&action=send&status=rejected`

---

## 6. Data Model

```sql
-- Organizations (multi-tenant)
CREATE TABLE organizations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    settings        JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- API keys
CREATE TABLE api_keys (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id),
    key_hash        BYTEA NOT NULL,           -- argon2 hash of pb_...
    key_prefix      TEXT NOT NULL,             -- first 8 chars for identification
    name            TEXT NOT NULL,
    permissions     TEXT[] NOT NULL DEFAULT '{}',
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_api_keys_prefix ON api_keys(key_prefix);

-- Inboxes (unified: native + linked)
CREATE TABLE inboxes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id),
    inbox_id        TEXT NOT NULL UNIQUE,      -- "agent@domain.com" or "linked:gmail:user@gmail.com"
    inbox_type      TEXT NOT NULL,             -- 'native' | 'linked_gmail' | 'linked_imap'
    username        TEXT,
    domain          TEXT,
    display_name    TEXT,
    client_id       TEXT,                      -- idempotency key
    metadata        JSONB NOT NULL DEFAULT '{}',
    permissions     JSONB NOT NULL DEFAULT '{}',
    -- Linked inbox fields
    provider_config JSONB,                     -- encrypted OAuth tokens / IMAP creds
    sync_interval   INTERVAL DEFAULT '5 minutes',
    last_sync_at    TIMESTAMPTZ,
    sync_status     TEXT DEFAULT 'idle',       -- 'idle' | 'syncing' | 'error'
    -- Trust tracking
    trust_score     INTEGER NOT NULL DEFAULT 0,
    trust_level     TEXT NOT NULL DEFAULT 'shadow', -- 'shadow' | 'approval' | 'auto_replies' | 'auto'
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_inboxes_org ON inboxes(org_id);

-- Threads
CREATE TABLE threads (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id        UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    thread_id       TEXT NOT NULL,
    subject         TEXT,
    labels          TEXT[] NOT NULL DEFAULT '{}',
    senders         TEXT[] NOT NULL DEFAULT '{}',
    recipients      TEXT[] NOT NULL DEFAULT '{}',
    preview         TEXT,
    last_message_id TEXT,
    message_count   INTEGER NOT NULL DEFAULT 0,
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(inbox_id, thread_id)
);
CREATE INDEX idx_threads_inbox ON threads(inbox_id, updated_at DESC);

-- Messages
CREATE TABLE messages (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id        UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    thread_id       UUID REFERENCES threads(id) ON DELETE CASCADE,
    message_id      TEXT NOT NULL,             -- RFC 2822 Message-ID
    provider_uid    TEXT,                      -- IMAP UID or Gmail message ID (for sync)
    labels          TEXT[] NOT NULL DEFAULT '{}',
    direction       TEXT NOT NULL,             -- 'inbound' | 'outbound'
    from_addr       TEXT NOT NULL,
    to_addrs        TEXT[] NOT NULL DEFAULT '{}',
    cc_addrs        TEXT[] NOT NULL DEFAULT '{}',
    bcc_addrs       TEXT[] NOT NULL DEFAULT '{}',
    reply_to        TEXT,
    subject         TEXT,
    text_body       TEXT,
    html_body       TEXT,
    extracted_text  TEXT,                      -- reply content without quoted history
    extracted_html  TEXT,
    preview         TEXT,
    in_reply_to     TEXT,                      -- threading
    references      TEXT[] NOT NULL DEFAULT '{}',
    headers         JSONB,                     -- raw headers for deep inspection
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    raw_message     BYTEA,                     -- optional: store full RFC 5322 message
    embedding       vector(1536),              -- pgvector for semantic search
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(inbox_id, message_id)
);
CREATE INDEX idx_messages_inbox ON messages(inbox_id, created_at DESC);
CREATE INDEX idx_messages_thread ON messages(thread_id, created_at ASC);
CREATE INDEX idx_messages_provider ON messages(inbox_id, provider_uid);
CREATE INDEX idx_messages_embedding ON messages USING ivfflat (embedding vector_cosine_ops);

-- Attachments
CREATE TABLE attachments (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    message_id      UUID NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename        TEXT NOT NULL,
    content_type    TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    storage_key     TEXT NOT NULL,             -- path in local FS or S3 key
    content_id      TEXT,                      -- for inline images (CID)
    is_inline       BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Drafts
CREATE TABLE drafts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id        UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    thread_id       UUID REFERENCES threads(id),
    to_addrs        TEXT[] NOT NULL DEFAULT '{}',
    cc_addrs        TEXT[] NOT NULL DEFAULT '{}',
    bcc_addrs       TEXT[] NOT NULL DEFAULT '{}',
    subject         TEXT,
    text_body       TEXT,
    html_body       TEXT,
    attachments     JSONB DEFAULT '[]',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Webhooks
CREATE TABLE webhooks (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id),
    url             TEXT NOT NULL,
    secret          TEXT NOT NULL,             -- HMAC-SHA256 signing key
    events          TEXT[] NOT NULL DEFAULT '{}',
    active          BOOLEAN NOT NULL DEFAULT true,
    failure_count   INTEGER NOT NULL DEFAULT 0,
    last_failure    TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Domains
CREATE TABLE domains (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID NOT NULL REFERENCES organizations(id),
    domain          TEXT NOT NULL UNIQUE,
    status          TEXT NOT NULL DEFAULT 'pending', -- 'pending' | 'verified' | 'failed'
    dns_records     JSONB NOT NULL DEFAULT '[]',
    dkim_selector   TEXT,
    dkim_private    BYTEA,                     -- encrypted DKIM private key
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Pending sends (approval workflow)
CREATE TABLE pending_sends (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id        UUID NOT NULL REFERENCES inboxes(id),
    draft_id        UUID REFERENCES drafts(id),
    to_addrs        TEXT[] NOT NULL,
    subject         TEXT,
    text_body       TEXT,
    html_body       TEXT,
    status          TEXT NOT NULL DEFAULT 'pending', -- 'pending' | 'approved' | 'rejected' | 'expired'
    auto_rule_match TEXT,                      -- which auto-approve rule matched, if any
    resolved_by     TEXT,                      -- 'human' | 'auto_rule' | 'timeout'
    resolved_at     TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Audit log (append-only)
CREATE TABLE audit_log (
    id              BIGSERIAL PRIMARY KEY,
    org_id          UUID NOT NULL,
    inbox_id        UUID,
    action          TEXT NOT NULL,
    status          TEXT NOT NULL,
    details         JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_audit_inbox ON audit_log(inbox_id, created_at DESC);

-- Sync queue (for linked inbox write-back operations)
CREATE TABLE sync_queue (
    id              BIGSERIAL PRIMARY KEY,
    inbox_id        UUID NOT NULL REFERENCES inboxes(id),
    operation       TEXT NOT NULL,             -- 'mark_read' | 'label' | 'archive' | 'delete' | 'send'
    target_uid      TEXT,                      -- provider message UID
    payload         JSONB NOT NULL DEFAULT '{}',
    status          TEXT NOT NULL DEFAULT 'pending',
    retry_count     INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_sync_queue_pending ON sync_queue(inbox_id, status) WHERE status = 'pending';
```

---

## 7. API Design

### Authentication

```
Authorization: Bearer pb_live_a1b2c3d4e5f6...
```

Keys are prefixed `pb_live_` (production) or `pb_test_` (sandbox). The prefix is stored in plaintext for lookup; the full key is argon2-hashed.

### Core endpoints

```
# Inboxes
POST   /api/v1/inboxes                           # Create native inbox
POST   /api/v1/inboxes/link                       # Link external account
GET    /api/v1/inboxes                            # List all inboxes
GET    /api/v1/inboxes/:id                        # Get inbox details
PATCH  /api/v1/inboxes/:id                        # Update inbox
DELETE /api/v1/inboxes/:id                        # Delete inbox
POST   /api/v1/inboxes/:id/sync                   # Force sync (linked only)
GET    /api/v1/inboxes/:id/sync/status             # Sync status

# Messages
POST   /api/v1/inboxes/:id/messages/send          # Send (or queue for approval)
GET    /api/v1/inboxes/:id/messages                # List messages
GET    /api/v1/inboxes/:id/messages/:msg_id        # Get message
DELETE /api/v1/inboxes/:id/messages/:msg_id        # Delete message

# Threads
GET    /api/v1/inboxes/:id/threads                 # List threads
GET    /api/v1/inboxes/:id/threads/:thread_id      # Get thread + messages

# Drafts
POST   /api/v1/inboxes/:id/drafts                  # Create draft
GET    /api/v1/inboxes/:id/drafts                   # List drafts
PATCH  /api/v1/inboxes/:id/drafts/:draft_id         # Update draft
POST   /api/v1/inboxes/:id/drafts/:draft_id/send    # Send draft
DELETE /api/v1/inboxes/:id/drafts/:draft_id          # Delete draft

# Labels
POST   /api/v1/inboxes/:id/labels                   # Create label
GET    /api/v1/inboxes/:id/labels                    # List labels
PUT    /api/v1/inboxes/:id/messages/:msg_id/labels   # Set message labels

# Attachments
GET    /api/v1/attachments/:attachment_id             # Download attachment
POST   /api/v1/attachments                            # Upload attachment

# Search
POST   /api/v1/search                                # Semantic search

# Domains
POST   /api/v1/domains                               # Add domain
GET    /api/v1/domains                                # List domains
POST   /api/v1/domains/:id/verify                     # Verify DNS

# Webhooks
POST   /api/v1/webhooks                               # Register
GET    /api/v1/webhooks                                # List
PATCH  /api/v1/webhooks/:id                            # Update
DELETE /api/v1/webhooks/:id                             # Delete

# Approvals (human actions)
GET    /api/v1/approvals                               # List pending
POST   /api/v1/approvals/:id/approve                   # Approve send
POST   /api/v1/approvals/:id/reject                    # Reject send

# Permissions
GET    /api/v1/inboxes/:id/permissions                 # Get permission config
PUT    /api/v1/inboxes/:id/permissions                 # Update permissions

# Audit
GET    /api/v1/audit                                   # Query audit log

# Metrics
GET    /api/v1/metrics                                 # Usage stats

# Health
GET    /health                                         # Liveness
GET    /ready                                          # Readiness (DB + Stalwart)
```

### Link external account

```json
POST /api/v1/inboxes/link
{
  "provider": "gmail",              // "gmail" | "outlook" | "imap"
  "display_name": "My Work Email",
  "sync_interval_seconds": 300,
  // For Gmail:
  "oauth_code": "4/0AX4...",       // from OAuth consent flow
  // For generic IMAP:
  "imap_host": "imap.fastmail.com",
  "imap_port": 993,
  "imap_username": "user@fastmail.com",
  "imap_password": "app-password",
  // Permissions
  "permissions": {
    "read": true,
    "label": true,
    "archive": true,
    "draft": true,
    "delete": false,
    "send": { "mode": "approval" }
  }
}
```

---

## 8. Email Parsing Pipeline

Every inbound email (native or synced) goes through this pipeline:

```
Raw RFC 5322 bytes
  │
  ▼
┌──────────────────────────────────┐
│ 1. Parse (mail-parser)            │
│    → Headers, body parts,         │
│      attachments, character set   │
│      decoding, MIME tree          │
└──────────────┬───────────────────┘
               │
  ▼
┌──────────────────────────────────┐
│ 2. Thread assignment              │
│    → Match In-Reply-To/References │
│      against existing threads     │
│    → Create new thread if none    │
│    → Subject-based fallback       │
└──────────────┬───────────────────┘
               │
  ▼
┌──────────────────────────────────┐
│ 3. Reply extraction               │
│    → Strip quoted text            │
│    → Strip signatures             │
│    → Populate extracted_text/html │
└──────────────┬───────────────────┘
               │
  ▼
┌──────────────────────────────────┐
│ 4. Attachment extraction          │
│    → Store files to disk/S3       │
│    → Record metadata in DB        │
│    → Handle inline images (CID)   │
└──────────────┬───────────────────┘
               │
  ▼
┌──────────────────────────────────┐
│ 5. Embedding generation           │
│    → text_body → embedding model  │
│    → Store vector in pgvector     │
│    → (async, non-blocking)        │
└──────────────┬───────────────────┘
               │
  ▼
┌──────────────────────────────────┐
│ 6. Event dispatch                 │
│    → WebSocket broadcast          │
│    → Webhook HTTP POST            │
│    → Pending approval check       │
└──────────────────────────────────┘
```

### Supported email types

| Type | Handling |
|------|----------|
| text/plain | Direct storage |
| text/html | Stored + sanitized preview |
| multipart/alternative | Both text + HTML extracted |
| multipart/mixed | Body + attachments separated |
| multipart/related | Inline images linked via CID |
| message/rfc822 | Nested messages parsed recursively |
| text/calendar (ICS) | Parsed as attachment + structured event data |
| application/pgp-encrypted | Stored encrypted, flag set |
| application/pkcs7-mime (S/MIME) | Stored encrypted, flag set |
| multipart/report (DSN) | Delivery status parsed, bounce event emitted |
| application/ms-tnef (winmail.dat) | Best-effort extraction (tnef crate or skip) |

All of this is handled by `mail-parser` out of the box, which supports RFC 5322, MIME (RFC 2045-2049), and 41 character encodings. We don't need to build any of this.

---

## 9. MCP Server

Tools exposed to AI agents via MCP:

```
# Inbox management
postbox.inbox.list             → list all inboxes
postbox.inbox.create           → create native inbox
postbox.inbox.link             → link external account

# Reading
postbox.messages.list          → list messages (with filters)
postbox.messages.get           → get single message with full body
postbox.threads.list           → list threads
postbox.threads.get            → get thread with all messages
postbox.search                 → semantic search

# Writing
postbox.messages.send          → send email (respects permissions)
postbox.messages.reply         → reply to a thread
postbox.drafts.create          → create draft
postbox.drafts.send            → send existing draft

# Organization
postbox.labels.add             → add label to message
postbox.labels.remove          → remove label
postbox.messages.archive       → archive message
postbox.messages.delete        → delete message (if permitted)

# Meta
postbox.permissions.get        → check what this agent can do
postbox.approvals.list         → list pending approvals (for human-facing agents)
```

Transport: stdio (for Claude Code / Cursor) and SSE (for web-based agents).

---

## 10. Configuration

Two tiers: simple TOML for most users, deep config for power users.

### Basic config (postbox.toml)

```toml
[server]
host = "0.0.0.0"
port = 4000
api_key = "pb_live_..."

[database]
url = "postgres://postbox:secret@localhost:5432/postbox"

[stalwart]
api_url = "http://localhost:8080"
api_key = "stalwart-admin-key"
default_domain = "yourdomain.com"

[storage]
type = "local"               # "local" | "s3"
path = "/var/lib/postbox/attachments"

[embeddings]
enabled = true
provider = "ollama"          # "ollama" | "openai" | "none"
model = "nomic-embed-text"
url = "http://localhost:11434"

[sync]
default_interval = "5m"
max_concurrent = 10

[notifications]
# Where to send approval notifications
type = "webhook"             # "webhook" | "ntfy" | "pushover" | "email"
url = "https://ntfy.sh/your-topic"
```

### Deep config (hooks + Sieve)

```toml
# Custom processing hooks — run your own logic on email events
[hooks.on_message_received]
exec = "/usr/local/bin/my-triage-script"
timeout = "5s"
# Script receives JSON on stdin, returns JSON with actions

[hooks.on_before_send]
exec = "/usr/local/bin/my-compliance-check"
timeout = "10s"
# Return {"allow": true/false, "reason": "..."}

# Sieve scripts for mail filtering (Stalwart supports these natively)
[sieve]
enabled = true
scripts_dir = "/etc/postbox/sieve/"
```

### Web dashboard (for non-technical users)

A minimal web UI (served by PostBox itself, no separate frontend build):

- **Connect accounts:** "Connect Gmail" button → OAuth flow → done
- **Inbox overview:** See all inboxes, last activity, sync status
- **Approval queue:** Pending sends with approve/reject buttons
- **Activity feed:** What did agents do today?
- **Permission toggles:** Visual sliders for read/write/send/delete
- **Search:** Semantic search across all inboxes

Built as server-rendered HTML with htmx for interactivity. No React, no npm, no build step. Ships inside the single binary.

---

## 11. Project Structure

```
postbox/
├── Cargo.toml
├── Cargo.lock
├── postbox.toml.example
├── Dockerfile
├── docker-compose.yml
├── LICENSE                          # MIT
├── README.md
│
├── src/
│   ├── main.rs                      # Entry point, CLI args
│   ├── config.rs                    # TOML config loading
│   ├── app.rs                       # Application state, startup
│   │
│   ├── api/                         # HTTP layer (Axum handlers)
│   │   ├── mod.rs
│   │   ├── router.rs                # Route definitions
│   │   ├── auth.rs                  # API key middleware
│   │   ├── error.rs                 # Error types → JSON responses
│   │   ├── inboxes.rs
│   │   ├── messages.rs
│   │   ├── threads.rs
│   │   ├── drafts.rs
│   │   ├── labels.rs
│   │   ├── domains.rs
│   │   ├── webhooks.rs
│   │   ├── search.rs
│   │   ├── approvals.rs
│   │   ├── audit.rs
│   │   └── metrics.rs
│   │
│   ├── core/                        # Business logic (no HTTP knowledge)
│   │   ├── mod.rs
│   │   ├── inbox.rs                 # Inbox CRUD, unified over native + linked
│   │   ├── message.rs               # Message processing pipeline
│   │   ├── thread.rs                # Thread assignment from headers
│   │   ├── draft.rs
│   │   ├── label.rs
│   │   ├── attachment.rs
│   │   ├── domain.rs
│   │   └── search.rs                # Semantic search queries
│   │
│   ├── permissions/                 # Permission engine
│   │   ├── mod.rs
│   │   ├── engine.rs                # Check + enforce permissions
│   │   ├── rules.rs                 # Auto-approval rule evaluation
│   │   ├── trust.rs                 # Progressive trust scoring
│   │   └── types.rs
│   │
│   ├── sync/                        # External account sync
│   │   ├── mod.rs
│   │   ├── engine.rs                # Sync orchestrator (per-account tasks)
│   │   ├── imap.rs                  # Generic IMAP sync + IDLE
│   │   ├── gmail.rs                 # Gmail API sync (OAuth + REST)
│   │   ├── reconcile.rs             # Conflict detection + resolution
│   │   └── queue.rs                 # Write-back operation queue
│   │
│   ├── stalwart/                    # Stalwart integration
│   │   ├── mod.rs
│   │   ├── client.rs                # REST API client
│   │   ├── accounts.rs              # Account CRUD
│   │   └── webhooks.rs              # Receive Stalwart webhook events
│   │
│   ├── mail/                        # Email processing
│   │   ├── mod.rs
│   │   ├── parser.rs                # Wrapper around mail-parser
│   │   ├── builder.rs               # Wrapper around mail-builder
│   │   ├── reply_extract.rs         # Quote/signature stripping
│   │   └── threading.rs             # In-Reply-To / References logic
│   │
│   ├── events/                      # Event system
│   │   ├── mod.rs
│   │   ├── types.rs                 # Event type definitions
│   │   ├── dispatcher.rs            # Fan-out to WS + webhooks
│   │   ├── webhooks.rs              # HTTP delivery with retries + HMAC
│   │   └── websocket.rs             # WebSocket hub (tokio broadcast)
│   │
│   ├── mcp/                         # MCP server
│   │   ├── mod.rs
│   │   ├── server.rs                # JSON-RPC protocol handling
│   │   ├── tools.rs                 # Tool definitions + handlers
│   │   ├── stdio.rs                 # stdio transport
│   │   └── sse.rs                   # SSE transport
│   │
│   ├── embeddings/                  # Vector embedding providers
│   │   ├── mod.rs
│   │   ├── trait.rs                 # trait EmbeddingProvider
│   │   ├── ollama.rs
│   │   ├── openai.rs
│   │   └── noop.rs                  # Disabled / no embedding
│   │
│   ├── storage/                     # File storage backends
│   │   ├── mod.rs
│   │   ├── trait.rs                 # trait StorageBackend
│   │   ├── local.rs                 # Local filesystem
│   │   └── s3.rs                    # S3-compatible
│   │
│   ├── dashboard/                   # Web UI
│   │   ├── mod.rs
│   │   ├── routes.rs
│   │   ├── templates/               # HTML templates (askama or maud)
│   │   └── static/                  # CSS, htmx.min.js
│   │
│   ├── db/                          # Database layer
│   │   ├── mod.rs
│   │   ├── pool.rs                  # Connection pool setup
│   │   └── migrations/              # SQL migrations
│   │
│   └── audit/                       # Audit logging
│       ├── mod.rs
│       └── logger.rs
│
├── migrations/                      # SQLx migrations
│   ├── 001_initial.sql
│   ├── 002_linked_inboxes.sql
│   └── 003_permissions.sql
│
├── tests/
│   ├── api/
│   ├── sync/
│   ├── permissions/
│   └── fixtures/                    # Sample .eml files for parsing tests
│
├── sdk/                             # Client SDKs (separate crates / packages)
│   ├── python/
│   ├── typescript/
│   └── rust/
│
└── deploy/
    ├── postbox.service              # systemd unit
    ├── nginx.conf.example
    └── caddy.example
```

---

## 12. Phased Roadmap

### Phase 0 — Foundation (Weeks 1-2)

**Goal:** Binary that starts, connects to Postgres + Stalwart, serves health endpoints.

```
[ ] cargo init with workspace structure
[ ] Config loading (postbox.toml)
[ ] Database connection pool (sqlx + Postgres)
[ ] Migrations runner
[ ] Stalwart API client (health check, list accounts)
[ ] Axum server with /health and /ready
[ ] API key auth middleware
[ ] Docker Compose (PostBox + Stalwart + Postgres)
[ ] CI: cargo test + clippy + fmt
```

**Deliverable:** `docker compose up` gives you a running PostBox connected to Stalwart.

---

### Phase 1 — Native Inboxes + Core API (Weeks 3-5)

**Goal:** Feature parity with AgentMail's basic flow — create inbox, send email, receive email.

```
[ ] Inbox CRUD (create via Stalwart API, store metadata in PG)
[ ] Idempotent inbox creation (client_id)
[ ] Send message (build with mail-builder → submit via Stalwart SMTP)
[ ] Receive message (Stalwart webhook → parse with mail-parser → store)
[ ] Full email parsing pipeline (MIME, attachments, all content types)
[ ] Thread assignment (In-Reply-To / References header matching)
[ ] Reply extraction (strip quotes + signatures)
[ ] Attachment storage (local filesystem)
[ ] List/get messages, threads
[ ] Basic webhook dispatch (message.received, message.sent)
[ ] WebSocket events (tokio broadcast channels)
```

**Deliverable:** An agent can create an inbox, send emails, receive replies, and follow threaded conversations. Webhooks notify on new messages.

---

### Phase 2 — Labels, Drafts, Domains, Search (Weeks 6-8)

**Goal:** Full AgentMail API parity.

```
[ ] Label CRUD + assign to messages
[ ] Drafts CRUD + send draft
[ ] Domain management (add domain, generate DNS records, verify)
[ ] DKIM key generation and Stalwart configuration
[ ] All webhook event types (delivered, bounced, complained, rejected)
[ ] Webhook HMAC signing + verification
[ ] Semantic search (pgvector + Ollama embedding)
[ ] Configurable embedding provider trait
[ ] Idempotency for all write operations
[ ] Pagination (cursor-based) on all list endpoints
[ ] Python SDK (auto-generated from OpenAPI spec or hand-written)
```

**Deliverable:** Full AgentMail feature parity. Python SDK works.

---

### Phase 3 — Linked Inboxes (Weeks 9-12)

**Goal:** Connect Gmail / IMAP accounts. Agent reads and acts on your real email.

```
[ ] OAuth2 flow for Gmail (web dashboard endpoint)
[ ] Gmail API sync: pull messages, labels, threads
[ ] Generic IMAP sync: connect to any IMAP server
[ ] IMAP IDLE for real-time new message notifications
[ ] Local message cache in Postgres (same schema as native)
[ ] Sync engine: per-account Tokio tasks
[ ] Configurable sync interval
[ ] Write-back queue: local operations → push to provider
[ ] Conflict detection + resolution
[ ] Sync status API endpoint
[ ] Force-sync endpoint
[ ] Encrypted credential storage (age or ring)
```

**Deliverable:** "Connect Gmail" works. Agent reads your real inbox. Actions sync back.

---

### Phase 4 — Permission Engine (Weeks 13-15)

**Goal:** Granular, auditable control over what agents can do.

```
[ ] Permission types: read, label, archive, draft, send, delete
[ ] Send modes: auto, approval, shadow, disabled
[ ] Approval workflow: pending_sends table + approve/reject API
[ ] Approval notifications: webhook, WebSocket, ntfy/Pushover
[ ] Auto-approval rules: domain allowlists, reply-to-existing, rate limits
[ ] Progressive trust: shadow → approval → auto (configurable thresholds)
[ ] Audit log: every agent action recorded
[ ] Audit query API
[ ] Permission config via API + TOML
```

**Deliverable:** Non-technical users can set "approval required for sends." Trust builds over time.

---

### Phase 5 — MCP + Dashboard + Hooks (Weeks 16-19)

**Goal:** Frictionless agent integration + human oversight UI + extensibility.

```
[ ] MCP server: stdio transport (Claude Code, Cursor)
[ ] MCP server: SSE transport (web-based agents)
[ ] All MCP tools (see Section 9)
[ ] Web dashboard: inbox overview, activity feed
[ ] Dashboard: "Connect Gmail" OAuth flow
[ ] Dashboard: approval queue with approve/reject buttons
[ ] Dashboard: permission toggles
[ ] Dashboard: search interface
[ ] Hook system: on_message_received, on_before_send exec hooks
[ ] Sieve script pass-through to Stalwart
[ ] TypeScript SDK
```

**Deliverable:** Non-technical users manage everything from a web dashboard. Claude Code users add PostBox as an MCP server and go.

---

### Phase 6 — Hardening + v1.0 (Weeks 20-24)

**Goal:** Production-ready, documented, community-launchable.

```
[ ] Multi-tenancy with org isolation
[ ] Rate limiting (configurable per-org, per-inbox)
[ ] Bounce tracking + auto-suppression
[ ] Batch operations (bulk label, bulk archive)
[ ] S3-compatible storage backend
[ ] Metrics endpoint (Prometheus-compatible)
[ ] OpenAPI spec generation
[ ] Comprehensive docs site (mdbook or similar)
[ ] Kubernetes manifests + Helm chart
[ ] Security audit of auth, credential storage, webhook signing
[ ] Performance benchmarks: messages/sec parsing, API latency p99
[ ] Fuzz testing on email parsing pipeline
[ ] Rust SDK (client library)
[ ] GitHub Actions: release binaries for linux-amd64, linux-arm64, macOS
[ ] README, CONTRIBUTING, architecture docs
[ ] v1.0 release
```

---

## 13. Performance Targets

| Metric | Target | How |
|--------|--------|-----|
| Idle memory | < 10 MB | Rust binary, no runtime overhead |
| API latency (cached) | < 5 ms p99 | In-process Postgres pool, zero-copy parsing |
| Email parse throughput | > 5,000 msgs/sec | mail-parser zero-copy + rayon for batch |
| Concurrent linked syncs | 100+ accounts | Tokio tasks, 1 task per account |
| WebSocket fan-out | < 1 ms | tokio::sync::broadcast |
| Webhook dispatch | < 100 ms p95 | Async HTTP with bounded concurrency |
| Semantic search | < 50 ms | pgvector IVFFlat index |
| Docker image size | < 20 MB | Static musl binary + scratch base |
| Startup time | < 1 second | No JIT, no interpreter |

---

## 14. What PostBox Has That AgentMail Doesn't

| Feature | AgentMail | PostBox |
|---------|-----------|---------|
| Self-hosted | No | Yes |
| Linked Gmail / IMAP sync | No | Yes |
| Permission engine | No | Yes (4 send modes) |
| Progressive trust | No | Yes |
| Approval workflows | No | Yes |
| Audit log | No | Yes |
| Hook system | No | Yes (exec hooks) |
| TOML config | No (SaaS) | Yes |
| Web dashboard | Console only | Built-in |
| Offline / local first | No | Yes |
| Cost | $0-500/mo | $0 (your infra) |

---

## 15. Naming + Open Source Strategy

**Name:** PostBox (or alternatives: **SentBox**, **AgentPost**, **MailForge**)

**License:** MIT — maximum adoption, no friction for commercial use.

**Repo structure:** Monorepo with `sdk/python`, `sdk/typescript`, `sdk/rust` subdirectories.

**Community:** Discord for support. GitHub Discussions for features. Good first issues tagged for new contributors.

**Positioning:** "The self-hosted AgentMail. Give your AI agents email with full control, linked account sync, and permission guardrails. One binary, zero cloud dependencies."
