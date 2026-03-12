# SlopGate — Vision & Migration Addendum

> From "email infrastructure for AI agents" to "AI-powered message firewall for humans."
> This document redefines the product, maps the migration from the PostBox specs, identifies
> new market opportunities, and defines features that didn't exist in the original plan.

---

## 1. The Pivot — What Changed and Why

### What we started building (PostBox)

An open-source, self-hosted clone of AgentMail. Give AI agents their own email inboxes.
Programmatic inbox creation, send/receive, threading, webhooks, MCP server. Rust for
performance. The positioning was: "Like AgentMail, but you own it."

### What we're actually building (SlopGate)

A self-hosted, AI-powered message firewall that sits between you and all your
communication channels. Your AI agent reads your email, texts, and notifications —
triages them, filters the slop, drafts responses, and surfaces only what matters.
The positioning is: **"Use AI to skip all the AI slop."**

### Why this is a better product

**PostBox competed with AgentMail on their turf** — developer infrastructure for agent
email. AgentMail has YC funding, 6 employees, real customers, and a head start.
We'd always be "the self-hosted alternative."

**SlopGate competes with nobody directly.** The closest things are:

- **Inbox Zero** (24K stars) — AI email assistant, but SaaS, Gmail-only, no SMS, no
  self-hosted, no MCP, no unified message layer
- **Superhuman** ($30/mo) — fast email client with AI, but closed-source, SaaS, email-only
- **SaneBox** — email triage SaaS, but no self-hosted, no agent integration, no SMS
- **Kagi SlopStop** — AI slop detection for search results, not email/messages
- **OpenClaw/Clawdbot** — general AI assistant with email as one plugin, not focused on
  message intelligence

Nobody is building a **self-hosted, privacy-first, multi-channel message firewall
with AI triage, agent integration, and slop detection.** The niche is genuinely empty.

### What stays the same

All the plumbing from the PostBox spec is unchanged:
- Rust + Axum + Tokio + PostgreSQL + pgvector
- Stalwart as the mail server backend
- mail-parser / mail-builder for email processing
- MIT license, same dependency strategy, same engineering principles
- Same performance targets (<10MB idle, <5ms API latency)
- Same project structure, CI/CD, release strategy

What changes is what sits on top of that plumbing, and how it's presented to users.

---

## 2. Product Identity

**Name:** SlopGate

**Tagline:** Use AI to skip the AI slop.

**One-liner:** Self-hosted message firewall. Your AI agent triages email, SMS, and
notifications — so you only see what matters.

**What it is for users:**
- An AI-powered layer between you and your inbox
- Reads all your email and texts as they arrive
- Scores every message: signal or slop?
- Auto-handles the slop (archive, delete, unsubscribe)
- Surfaces the signal (important emails, personal messages, action items)
- Drafts responses for routine emails, queues them for your approval
- Gives you a daily briefing: "here's what happened, here's what needs you"

**What it is for developers/agents:**
- Everything PostBox was — programmatic inboxes, send/receive, threading, webhooks, MCP
- Plus: unified message API across email + SMS + future channels
- Plus: slop scoring and classification built into every message
- Plus: triage actions (auto-archive, auto-label, auto-respond) as API operations

**What it is technically:**
- A Rust binary that connects to Stalwart (email), KDE Connect (SMS), and PostgreSQL
- Exposes a REST API, WebSocket events, and MCP server
- Runs a classification pipeline on every inbound message
- Stores everything in a unified message store with embeddings for search

---

## 3. New Market Position

### Who the users are (broader than PostBox)

**Tier 1: Linux power users / self-hosters** (launch audience)
- Already run Stalwart, Nextcloud, etc. on their homelab or VPS
- Use the terminal daily, appreciate Rust performance
- Privacy-conscious, want email on their own infrastructure
- THIS IS YOU — build for yourself first

**Tier 2: AI agent developers** (growth audience)
- Building agents that need email/messaging capabilities
- Currently using AgentMail or hacking Gmail APIs
- Want self-hosted + framework-agnostic + MCP-native
- The original PostBox audience — they're still served

**Tier 3: Knowledge workers drowning in email** (mass market, later)
- 100+ emails/day, most is slop (newsletters, AI outreach, notifications)
- Currently using Superhuman ($30/mo) or SaneBox ($7/mo) or nothing
- Would use a self-hosted alternative if setup was easy enough
- The web dashboard and Docker quick-start serve this audience

### The competitive landscape for SlopGate

| Product | Self-hosted | Multi-channel | Slop detection | Agent-native | MCP |
|---------|-----------|---------------|----------------|-------------|-----|
| **SlopGate** | ✓ | Email + SMS | ✓ | ✓ | ✓ |
| Inbox Zero | ✗ (SaaS) | Email only | Partial | ✗ | ✗ |
| Superhuman | ✗ (SaaS) | Email only | ✗ | ✗ | ✗ |
| SaneBox | ✗ (SaaS) | Email only | Partial | ✗ | ✗ |
| Shortwave | ✗ (SaaS) | Email only | ✗ | ✗ | ✗ |
| OpenClaw | ✓ | Multi (plugins) | ✗ | ✓ | ✓ |
| AgentMail | ✗ (SaaS) | Email only | ✗ | ✓ | ✓ |
| Kagi SlopStop | N/A | Search only | ✓ (search) | ✗ | ✗ |

**Key insight from Kagi:** Kagi launched SlopStop to detect and downrank AI slop in search
results. They're building "the largest dataset of AI-slop domains on the web." They frame
it as "an existential threat to an internet that should belong to humans." SlopGate is the
same philosophy applied to your inbox instead of your search results.

---

## 4. Unified Message Store — The Core Architectural Change

The PostBox spec had an email-specific `messages` table. SlopGate has a channel-agnostic
unified message store.

### Schema changes from PostBox

```sql
CREATE TABLE messages (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    inbox_id        UUID NOT NULL REFERENCES inboxes(id) ON DELETE CASCADE,
    thread_id       UUID REFERENCES threads(id),

    -- Channel identification (NEW)
    channel         TEXT NOT NULL,             -- 'email' | 'sms' | 'signal' | 'notification'
    provider        TEXT NOT NULL,             -- 'stalwart' | 'gmail' | 'imap' | 'kde-connect' | 'twilio'

    -- Universal fields (all channels)
    direction       TEXT NOT NULL,             -- 'inbound' | 'outbound'
    from_addr       TEXT NOT NULL,             -- email, phone number, or handle
    to_addrs        TEXT[] NOT NULL DEFAULT '{}',
    body_text       TEXT,
    body_html       TEXT,                      -- null for SMS
    preview         TEXT,
    timestamp       TIMESTAMPTZ NOT NULL,

    -- Email-specific (null for other channels)
    subject         TEXT,
    cc_addrs        TEXT[] NOT NULL DEFAULT '{}',
    bcc_addrs       TEXT[] NOT NULL DEFAULT '{}',
    reply_to        TEXT,
    in_reply_to     TEXT,
    references_     TEXT[] NOT NULL DEFAULT '{}',
    headers         JSONB,
    message_id      TEXT,                      -- RFC 2822 Message-ID

    -- SMS-specific (null for email)
    phone_from      TEXT,
    phone_to        TEXT,

    -- Reply extraction (email)
    extracted_text  TEXT,
    extracted_html  TEXT,

    -- === THE SLOPGATE LAYER (all NEW) ===

    -- Slop scoring
    slop_score      REAL,                      -- 0.0 = definitely important, 1.0 = pure slop
    slop_signals    JSONB DEFAULT '{}',        -- {"ai_generated": 0.8, "newsletter": 0.9, "cold_outreach": 0.7}

    -- Classification
    category        TEXT,                      -- 'personal' | 'work' | 'transactional' | 'newsletter'
                                               -- 'cold_outreach' | 'notification' | 'spam' | 'ai_slop'
    subcategory     TEXT,                      -- 'family' | 'friend' | 'client' | 'vendor' | 'unknown'
    priority        TEXT DEFAULT 'normal',     -- 'urgent' | 'high' | 'normal' | 'low' | 'none'

    -- Triage state
    triage_status   TEXT DEFAULT 'pending',    -- 'pending' | 'surfaced' | 'auto_handled' | 'dismissed'
    triage_action   TEXT,                      -- 'none' | 'archive' | 'delete' | 'label' | 'respond' | 'escalate'
    triaged_at      TIMESTAMPTZ,
    triaged_by      TEXT,                      -- 'auto_rule' | 'slop_classifier' | 'agent' | 'human'

    -- Action tracking
    requires_action BOOLEAN DEFAULT false,     -- needs human attention
    action_type     TEXT,                      -- 'reply_needed' | 'review_needed' | 'approval_needed' | 'fyi'
    action_deadline TIMESTAMPTZ,               -- optional: reply by this date

    -- Standard fields (from PostBox)
    size_bytes      BIGINT NOT NULL DEFAULT 0,
    raw_message     BYTEA,
    provider_uid    TEXT,
    labels          TEXT[] NOT NULL DEFAULT '{}',
    embedding       vector(1536),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes for triage queries
CREATE INDEX idx_messages_triage ON messages(inbox_id, triage_status, priority)
    WHERE triage_status = 'pending';
CREATE INDEX idx_messages_slop ON messages(inbox_id, slop_score)
    WHERE slop_score IS NOT NULL;
CREATE INDEX idx_messages_action ON messages(inbox_id, requires_action)
    WHERE requires_action = true;
```

### What the API returns now

Every message in SlopGate comes with triage metadata built in:

```json
GET /api/v1/messages/abc123

{
  "id": "abc123",
  "channel": "email",
  "from": "sales@randomstartup.io",
  "subject": "Quick question about your AI stack",
  "preview": "Hi Nikhil, I noticed your work on...",
  "body_text": "...",

  "slop_score": 0.87,
  "slop_signals": {
    "ai_generated_probability": 0.92,
    "cold_outreach_pattern": true,
    "sender_never_contacted_before": true,
    "generic_personalization": true
  },
  "category": "cold_outreach",
  "subcategory": "ai_slop",
  "priority": "none",

  "triage_status": "auto_handled",
  "triage_action": "archive",
  "triaged_by": "slop_classifier",

  "requires_action": false
}
```

---

## 5. The Slop Classification Pipeline

Every inbound message (email or SMS) runs through this pipeline:

```
Inbound message arrives
    │
    ▼
┌───────────────────────────────────┐
│ 1. Parse                          │
│    Email: mail-parser             │
│    SMS: plain text extraction     │
└───────────┬───────────────────────┘
            │
    ▼
┌───────────────────────────────────┐
│ 2. Sender reputation              │
│    Known sender? → check history  │
│    Unknown? → flag for scoring    │
│    In contacts? → boost priority  │
└───────────┬───────────────────────┘
            │
    ▼
┌───────────────────────────────────┐
│ 3. Rule-based classification      │  ← Fast, no LLM needed
│    Newsletter headers → newsletter│
│    Unsubscribe link → promotional │
│    Transactional patterns → txn   │
│    Known cold-email patterns      │
│    OTP/verification codes → txn   │
└───────────┬───────────────────────┘
            │
    ▼
┌───────────────────────────────────┐
│ 4. Slop scoring (optional LLM)    │  ← Ollama or API, configurable
│    AI-generated text detection    │
│    Generic personalization check  │
│    Template language patterns     │
│    Engagement bait detection      │
│    → slop_score: 0.0 to 1.0      │
└───────────┬───────────────────────┘
            │
    ▼
┌───────────────────────────────────┐
│ 5. Triage decision                │
│    slop_score > 0.8 → auto_archive│
│    slop_score > 0.95 → auto_delete│
│    requires_action → surface      │
│    from known contact → surface   │
│    Everything else → normal inbox │
└───────────┬───────────────────────┘
            │
    ▼
┌───────────────────────────────────┐
│ 6. Embed + store                  │
│    Generate embedding (async)     │
│    Store in unified message table │
│    Dispatch events (WS + webhook) │
└───────────────────────────────────┘
```

### Slop detection signals (rule-based, no LLM)

These run on every message with zero latency cost:

```
# Newsletter detection
- Has List-Unsubscribe header
- Has List-Id header
- From address matches known newsletter services (substack, mailchimp, etc.)
- Contains "unsubscribe" link in body

# Cold outreach / AI slop detection
- First contact from this sender
- Contains "I noticed your" / "I came across your" / "I was impressed by"
- References LinkedIn/Twitter profile (scraped personalization)
- From a recently registered domain
- Sender has low engagement history (never replied to)
- Body matches known AI-generated patterns (Kagi's SlopStop signal list)

# Transactional
- From a known service (GitHub, Stripe, AWS, etc.)
- Contains order number, tracking number, verification code
- Subject matches transactional patterns

# Personal
- From address in contacts
- Part of an existing thread with a human
- Short, informal language
- Contains questions directed at you specifically
```

### Slop detection with LLM (optional, for higher accuracy)

When an LLM is configured (Ollama locally, or API), run it on ambiguous messages:

```
System: You are a message classifier. Score how likely this message is AI-generated
slop (0.0 = definitely human-written and important, 1.0 = AI-generated junk).

Consider:
- Is this personalized to the recipient specifically, or generic?
- Does the language have the characteristic AI patterns (hedging, list-heavy, overly formal)?
- Is the sender trying to sell something or request a meeting without prior relationship?
- Would a busy professional want to read this or would they delete it instantly?

Return JSON: {"score": 0.0-1.0, "signals": {"ai_generated": bool, "cold_outreach": bool,
"genuine_personalization": bool, "action_required": bool}, "reason": "brief explanation"}
```

This runs async and doesn't block message delivery. The rule-based classifier handles
triage immediately; the LLM score refines it in the background.

---

## 6. SMS Integration via KDE Connect

### How it works on Fedora + Hyprland

```
Your phone receives SMS
    │
    ▼
KDE Connect Android app
    │ (local WiFi)
    ▼
kdeconnectd daemon on your Fedora machine
    │
    ▼ (D-Bus signal: org.kde.kdeconnect.device.*.sms.messagesChanged)
    │
SlopGate SMS provider module
    │
    ▼
Unified message store (same pipeline as email)
    │
    ▼
Available via API / MCP / TUI / Dashboard
```

### Implementation

SlopGate's SMS module subscribes to KDE Connect's D-Bus signals. When a new SMS arrives,
it's read via `kdeconnect-cli --read-sms` or the D-Bus conversation API, converted to
the unified message format, and stored in Postgres.

```rust
// Simplified — the actual D-Bus subscription
async fn watch_sms(dbus: Connection, tx: Sender<InboundMessage>) {
    let proxy = KdeConnectSmsProxy::new(&dbus, device_id).await?;
    let mut stream = proxy.receive_messages_changed().await?;

    while let Some(signal) = stream.next().await {
        let conversations = proxy.conversations().await?;
        for msg in conversations.new_messages() {
            tx.send(InboundMessage {
                channel: Channel::Sms,
                provider: Provider::KdeConnect,
                from_addr: msg.sender_phone().to_string(),
                body_text: Some(msg.body().to_string()),
                timestamp: msg.timestamp(),
                ..Default::default()
            }).await?;
        }
    }
}
```

### Sending SMS (optional, lower priority)

```bash
kdeconnect-cli --send-sms "message" --destination "+1234567890" --device <device-id>
```

SlopGate wraps this in the same send API:

```json
POST /api/v1/messages/send
{
  "channel": "sms",
  "to": "+1234567890",
  "text": "On my way, 10 minutes"
}
```

Same permission engine applies — if the inbox is in "approval" mode for SMS sends,
the message is queued for human review.

### Alternative SMS providers (future)

| Provider | How | Self-hosted? | Cost |
|----------|-----|-------------|------|
| **KDE Connect** | D-Bus, local WiFi | Yes, fully local | Free |
| **Twilio** | REST API | No (cloud) | Pay per message |
| **Google Voice** | Web scraping / email forwarding | Partial | Free |
| **Matrix/SMS bridge** | Matrix protocol | Yes | Free |
| **Android ADB** | USB/WiFi debug bridge | Yes, fully local | Free |

KDE Connect is the v1 choice. Others are Phase 7+ if there's demand.

---

## 7. New Features Unique to SlopGate

These features didn't exist in the PostBox spec because they only make sense
with the unified message + slop detection framing.

### 7.1 Daily Briefing

```
GET /api/v1/briefing?period=24h

{
  "period": "24h",
  "summary": {
    "total_messages": 47,
    "slop_auto_handled": 31,
    "surfaced_for_you": 8,
    "requires_your_action": 3,
    "drafts_awaiting_approval": 2,
    "sms_received": 3
  },
  "action_items": [
    {
      "id": "msg_abc",
      "channel": "email",
      "from": "john@acme.com",
      "subject": "Contract review needed",
      "action_type": "reply_needed",
      "priority": "high",
      "deadline": "2026-03-11T17:00:00Z"
    },
    {
      "id": "msg_def",
      "channel": "sms",
      "from": "+1234567890",
      "body": "Can you call me when you're free?",
      "action_type": "reply_needed",
      "priority": "high"
    }
  ],
  "drafts_ready": [
    {
      "id": "draft_123",
      "in_reply_to": "msg_ghi",
      "subject": "Re: Meeting confirmation",
      "preview": "Thanks for confirming. I'll be there at 3pm.",
      "confidence": 0.95
    }
  ],
  "slop_report": {
    "newsletters_archived": 12,
    "cold_outreach_deleted": 8,
    "ai_generated_blocked": 6,
    "promotional_archived": 5
  }
}
```

This is the endpoint that makes SlopGate useful as a daily productivity tool.
An agent (or the TUI, or the dashboard, or a morning notification) calls this
and presents: "Here's your morning. 3 things need you. 31 things were slop.
2 draft replies are ready for your review."

### 7.2 Slop Report / Analytics

```
GET /api/v1/analytics/slop?period=30d

{
  "period": "30d",
  "total_messages": 1420,
  "slop_percentage": 67.3,
  "breakdown": {
    "ai_generated_outreach": 312,
    "newsletters_unread": 245,
    "promotional": 198,
    "notification_noise": 147,
    "legitimate": 518
  },
  "top_slop_senders": [
    { "from": "*@salesforce-outreach.com", "count": 23, "avg_slop_score": 0.94 },
    { "from": "*@newsletter-platform.io", "count": 18, "avg_slop_score": 0.89 }
  ],
  "time_saved_estimate": "4.2 hours",
  "trend": "slop_increasing"
}
```

People love seeing how much garbage was filtered. It reinforces the value prop
every time they check. "SlopGate saved me 4 hours this month" is shareable.

### 7.3 Auto-Responder with Approval

SlopGate can draft responses to routine emails:

```
Meeting confirmation → "Thanks, confirmed for 3pm."
Receipt/invoice → no response needed, auto-archive
Simple question with obvious answer → draft response, queue for approval
Cold outreach → ignore (or optionally: polite decline)
```

Drafts appear in the approval queue. You review them in batch:

```
GET /api/v1/drafts/pending

[
  {
    "id": "draft_001",
    "in_reply_to": "msg_abc",
    "from": "you@domain.com",
    "to": "john@acme.com",
    "subject": "Re: Meeting Tuesday",
    "body": "Thanks John, 3pm works. See you then.",
    "confidence": 0.97,
    "category": "meeting_confirmation"
  },
  {
    "id": "draft_002",
    "in_reply_to": "msg_def",
    "from": "you@domain.com",
    "to": "sarah@client.co",
    "subject": "Re: Invoice Q3",
    "body": "Hi Sarah, received the invoice. I'll process it by Friday.",
    "confidence": 0.82,
    "category": "transactional_acknowledgment"
  }
]

POST /api/v1/drafts/draft_001/approve    → sends immediately
POST /api/v1/drafts/draft_002/edit       → opens for editing
POST /api/v1/drafts/batch-approve        → approve all high-confidence drafts
```

### 7.4 Personal Slop Training

SlopGate learns YOUR slop patterns over time:

```
POST /api/v1/feedback
{
  "message_id": "msg_xyz",
  "feedback": "slop"      // or "important" or "meh"
}
```

Over time, this builds a per-user classifier:
- "Nikhil always reads emails from @acme.com → boost priority"
- "Nikhil archives every Substack newsletter unread → auto-archive"
- "Nikhil deletes cold outreach within 2 seconds → auto-delete"
- "Nikhil always replies to SMS from +91... → mark high priority"

This is stored as a learned preferences model, not raw rules. It can be
as simple as sender-based heuristics early on, graduating to a fine-tuned
classifier later.

### 7.5 "Unslopify" View

A read-only view that strips out everything classified as slop:

```
GET /api/v1/messages?unslopify=true

// Returns only messages where:
// - slop_score < 0.5
// - OR from a known contact
// - OR marked requires_action
// - OR from an existing thread you've participated in
```

This is the "clean inbox" experience. Your TUI's default view.

### 7.6 Pulse Integration Hooks

SlopGate publishes events that Pulse (your desktop daemon) can subscribe to:

```
# Desktop notification via Pulse
{
  "event": "message.requires_action",
  "channel": "email",
  "from": "john@acme.com",
  "subject": "Contract review needed",
  "priority": "high"
}

# Morning briefing via Pulse
{
  "event": "daily_briefing",
  "action_items": 3,
  "slop_handled": 31,
  "drafts_ready": 2
}
```

Pulse shows a single notification: "3 messages need your attention. 31 handled."
Not 47 separate email notifications. This is the anti-notification-spam experience.

---

## 8. Revised Roadmap (SlopGate)

### Phase 0: Skeleton (1-2 days)
Same as PostBox. Binary starts, connects to Postgres + Stalwart, serves /health.

### Phase 1: First email + basic triage (3 weeks)
```
[ ] Inbox CRUD (native inboxes via Stalwart)
[ ] Gmail relay mode (zero-config quick start)
[ ] Send + receive messages
[ ] Email parsing pipeline (all MIME types)
[ ] Threading (In-Reply-To / References)
[ ] Reply extraction
[ ] Outbound guard (secret leak scanning)
[ ] ** Unified message store schema (channel-agnostic) **
[ ] ** Rule-based slop classification (no LLM) **
[ ] ** Slop score on every inbound message **
[ ] ** Auto-archive for slop_score > 0.8 **
[ ] Basic webhooks (message.received, message.triaged)
```

### Phase 2: Make triage useful (3 weeks)
```
[ ] Linked inbox sync (Gmail OAuth)
[ ] Labels, drafts
[ ] ** Daily briefing endpoint **
[ ] ** Unslopify view (filter mode on message list) **
[ ] ** Auto-responder with approval queue **
[ ] ** Slop analytics endpoint **
[ ] Interactive CLI shell
[ ] Python SDK
[ ] Framework examples (LangChain, CrewAI, Claude Code)
```

### Phase 3: SMS + intelligence (3 weeks)
```
[ ] ** KDE Connect SMS integration (D-Bus) **
[ ] ** SMS messages in unified store **
[ ] ** Slop scoring for SMS (spam detection) **
[ ] ** LLM-based slop scoring (Ollama, optional) **
[ ] ** Personal slop training (feedback loop) **
[ ] Semantic search (pgvector)
[ ] Generic IMAP linked inbox sync
[ ] TypeScript SDK
```

### Phase 4: Permissions + safety (2 weeks)
```
[ ] Permission engine (4 send modes)
[ ] Progressive trust
[ ] Approval workflows for sends
[ ] Notification providers (ntfy, Pushover, Telegram, email)
[ ] Audit log
```

### Phase 5: MCP + dashboard (3 weeks)
```
[ ] MCP server (stdio + SSE)
[ ] Web dashboard (server-rendered, htmx)
[ ] Dashboard: briefing view, triage queue, approval queue
[ ] Dashboard: "Connect Gmail" OAuth flow
[ ] Dashboard: slop analytics visualization
[ ] ** Pulse integration hooks **
[ ] Hook system (exec hooks for custom triage)
```

### Phase 6: TUI + v1.0 (3 weeks)
```
[ ] ** Superfile-style TUI (ratatui) **
[ ] ** TUI: multi-inbox view with slop filtering **
[ ] ** TUI: inline approve/reject drafts **
[ ] ** TUI: briefing view **
[ ] Multi-tenancy
[ ] Rate limiting, bounce tracking
[ ] Docs site, Kubernetes manifests
[ ] v1.0 release
```

---

## 9. What Migrates from PostBox Specs

| PostBox Document | Status | Changes |
|-----------------|--------|---------|
| Full Technical Spec | **Mostly valid** | Update data model to unified message store. Add `channel` field. Add slop scoring fields. Rename PostBox → SlopGate throughout. |
| Licensing & Independence | **Unchanged** | All licenses, dependency strategy, monetization advice applies identically. |
| Project Setup Guide | **Unchanged** | Same repo structure, CI/CD, community files. Update name references. |
| Engineering Principles | **Unchanged** | Anti-bloat rules, performance targets, coding ethos apply exactly. |
| Competitive Landscape | **Needs update** | Add Inbox Zero, Superhuman, SaneBox, Kagi SlopStop as competitors. Reframe positioning. |
| Spec Improvements | **Mostly valid** | Gmail relay, CLI shell, outbound guard, daily briefing all still apply. TUI vision is strengthened by slop filtering use case. |

### What's NEW (not in any PostBox doc)

- Unified message store schema (email + SMS + future channels)
- Slop classification pipeline (rule-based + optional LLM)
- Slop scoring on every message (0.0 to 1.0)
- Slop signals (ai_generated, cold_outreach, newsletter, etc.)
- Auto-triage actions (archive, delete, label based on slop score)
- Daily briefing endpoint
- Slop analytics / reporting
- Auto-responder with approval
- Personal slop training (feedback loop)
- "Unslopify" view
- KDE Connect SMS integration
- Pulse integration hooks
- SMS in unified message store

---

## 10. The Pitch

**For Hacker News / Reddit:**

"I built SlopGate — a self-hosted message firewall that uses AI to filter the AI slop
from your inbox. It reads your email and texts, scores every message for slop
(AI-generated outreach, newsletter noise, promotional garbage), auto-handles the junk,
and surfaces only what matters. Daily briefing tells you '3 things need you, 31 were slop.'
Single Rust binary, <10MB idle, MIT licensed. Also works as email infrastructure for AI
agents (create inboxes, send/receive, MCP server)."

**Why this pitch works:**
- "AI to filter the AI slop" is immediately resonant — everyone has this problem
- "Self-hosted" appeals to the HN/Reddit audience
- "Rust binary, <10MB" appeals to the performance/Linux crowd
- "Also works as agent infra" catches the developer audience
- The slop analytics ("SlopGate saved me 4 hours this month") is shareable content

**For the README:**

```
📬 SlopGate

Use AI to skip the AI slop.

Self-hosted message firewall. Your AI agent triages email and SMS —
so you only see what matters.

$ slopgate briefing

  ┌─ Morning Briefing ─────────────────────────────┐
  │                                                  │
  │  📬 47 messages received                         │
  │  🗑️  31 auto-handled (slop)                      │
  │  📌 3 need your attention                        │
  │  ✏️  2 draft replies ready for review             │
  │                                                  │
  │  Time saved: ~45 minutes                         │
  │                                                  │
  └──────────────────────────────────────────────────┘
```

---

## 11. Open Questions

Decisions that need to be made during development, not now:

1. **Should slop scoring be required or optional?** If someone just wants agent email
   infrastructure without triage, should SlopGate work as a pure PostBox? Probably yes —
   set `triage.enabled = false` in config and it becomes a clean email API.

2. **How aggressive should auto-triage defaults be?** Conservative (only auto-archive
   obvious spam) or aggressive (auto-archive anything > 0.7 slop score)? Probably
   start conservative and let users tune up.

3. **Should the LLM slop scorer run on every message or only ambiguous ones?**
   Running Ollama on every email is expensive. Better: rule-based handles 80% of
   classification, LLM only runs on messages where rules are uncertain (score 0.3-0.7).

4. **How much history should the daily briefing cover?** 24 hours by default,
   configurable. But should it also surface "old threads that went stale and might
   need a follow-up"? Probably a Phase 3 feature.

5. **Should SlopGate expose a "slop API" for other tools?** Like Kagi shares their
   SlopStop data, should SlopGate publish its slop sender database for others to use?
   Interesting for community building but not a launch feature.
