# PostBox — Spec Improvements & New Ideas

> Amendments to the core spec based on competitive analysis, architectural review,
> and new feature ideas. This document supplements the full technical spec and
> should be read alongside it.

---

## 1. Steal: Gmail Relay Mode (from AgenticMail)

**What:** Add a zero-config "relay mode" that uses Gmail's `+` sub-addressing
as the fastest path to first email.

**How it works:**
- User provides their Gmail address + app password
- PostBox creates agent addresses as `you+agentname@gmail.com`
- Outbound: PostBox sends via Gmail's SMTP (`smtp.gmail.com:587`)
- Inbound: PostBox polls Gmail IMAP for messages addressed to `+agentname`
  and routes them to the agent's local mailbox
- Reply-To header ensures replies flow back correctly

**Why steal this:**
- 2-minute setup. No domain, no DNS, no Stalwart outbound config.
- Drastically lowers the barrier to "first agent email sent."
- Gmail handles deliverability — emails land in inboxes, not spam.
- Perfect for the "try before you commit" experience.

**Limitations to document clearly:**
- `you+agentname@gmail.com` looks unprofessional
- Reveals your personal email address
- Gmail rate limits (500/day consumer, 2000/day Workspace)
- All agents share one Gmail account's reputation
- 30-second polling delay on inbound (unless we use IMAP IDLE on Gmail)

**Where this fits in the three-tier inbox model:**

```
Tier 1: Gmail Relay (zero config, 2 min setup, try the product)
  ↓ user likes it, wants proper addresses
Tier 2: Native Inbox (Stalwart, your domain, agent@yourdomain.com)
  ↓ user wants agent on their real email
Tier 3: Linked Inbox (Gmail/IMAP sync, agent operates on your inbox)
```

**Phase:** Add to Phase 1 alongside native inboxes. This becomes the
quick-start path in the README.

---

## 2. Steal: Interactive CLI Shell

**What:** A REPL-style interactive shell for managing PostBox from the
terminal, similar to AgenticMail's 44-command shell.

**Why:**
- Developers live in terminals. `postbox> /inbox` is faster than curl.
- Great for demos and onboarding — people can see their agent's email
  instantly without writing code.
- AgenticMail's shell with arrow key navigation, inline email reading,
  and pagination is genuinely good UX.

**Our version should include:**

```
# Core
/inbox                    List inbox (paginated, arrow key navigation)
/read <id>                Read email inline with threading
/send                     Interactive compose (to, subject, body)
/reply <id>               Reply to a message
/search <query>           Search across inboxes
/threads                  List conversation threads

# Management
/inboxes                  List all inboxes (native + linked + relay)
/create <name>            Create new native inbox
/link gmail               Start Gmail OAuth flow / relay setup
/sync <inbox>             Force sync a linked inbox
/status                   Show server health, Stalwart status, sync status

# Permissions
/approvals                List pending sends awaiting approval
/approve <id>             Approve a pending send
/reject <id>              Reject a pending send
/trust <inbox>            Show trust score and permission config

# Admin
/agents                   List all agent inboxes
/audit                    Recent audit log entries
/metrics                  Usage stats
/config                   Show current config
```

**Implementation:** Use `ratatui` (Rust TUI framework) for the shell.
Runs inside the same binary — `postbox shell` launches it.

**Phase:** Phase 2 — after core API works, before dashboard.

---

## 3. New: Superfile-Style TUI Email Client (`postbox tui`)

**What:** A full terminal UI for PostBox inspired by Superfile's multi-panel
design, but for email instead of files. Think "Superfile meets Mutt" — modern,
beautiful, keyboard-driven email management for all your agent inboxes.

**Why this is compelling:**
- Superfile proved that terminal tools can be visually beautiful with
  Nerd Font icons, color themes, and polished layouts
- No TUI email client exists that's designed for multi-inbox agent management
- Fits perfectly with the Linux/terminal-native audience PostBox targets
- You already use Hyprland + terminal workflows — this is built for people like you
- A stunning TUI gets GitHub stars. People share screenshots of beautiful terminals.

**Design vision:**

```
┌─ PostBox ──────────────────────────────────────────────────────────────┐
│                                                                        │
│  ┌─ Inboxes ──────┐  ┌─ sales-agent@domain.com ───────────────────┐  │
│  │                 │  │                                             │  │
│  │ 📬 sales-agent │  │  ★ john@acme.com        Re: Q3 Proposal  │  │
│  │ 📬 support-bot │  │    sarah@client.co      Meeting Tuesday   │  │
│  │ 🔗 My Gmail    │  │    dev@stripe.com       Invoice #4521     │  │
│  │ 🔗 Work Gmail  │  │  ★ mike@vendor.io       Parts shipment    │  │
│  │                 │  │    notifications@gh..   [repo] PR #142    │  │
│  │ ─── Relay ──── │  │                                             │  │
│  │ 📨 quick-test  │  │                                             │  │
│  │                 │  │                                             │  │
│  │                 │  │                                             │  │
│  │ [n]ew [d]elete │  │  Page 1/3  [j/k] navigate  [Enter] read   │  │
│  └─────────────────┘  └─────────────────────────────────────────────┘  │
│                                                                        │
│  ┌─ Preview ───────────────────────────────────────────────────────┐  │
│  │ From: john@acme.com                                              │  │
│  │ Subject: Re: Q3 Proposal                                        │  │
│  │ Date: 2026-03-10 14:32                                          │  │
│  │ ────────────────────────────────────────────────────────────     │  │
│  │ Hi, thanks for sending over the proposal. I had a few           │  │
│  │ questions about the pricing on page 3...                        │  │
│  │                                                                  │  │
│  │ [r]eply  [f]orward  [a]rchive  [d]elete  [l]abel  [s]earch    │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                                                        │
│  ┌─ Status ────────────────────────────────────────────────────────┐  │
│  │ 📊 5 inboxes │ 🔄 Gmail synced 30s ago │ ⏳ 2 pending approvals│  │
│  └──────────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

**Key features:**

- **Multi-panel layout** — inbox list on left, message list center, preview below
  (exactly like Superfile's file panels but for email)
- **Multiple inbox view** — see all inboxes (native, linked, relay) in one sidebar,
  switch between them with Tab or click
- **Vim keybindings** — j/k navigate, Enter opens, r replies, / searches
- **Nerd Font icons** — 📬 for native, 🔗 for linked, 📨 for relay
- **Color themes** — ship with Nord, Dracula, Catppuccin, Tokyo Night
  (same themes the terminal community loves)
- **Live updates** — new emails appear in real-time via WebSocket
- **Inline compose** — write replies without leaving the TUI
- **Thread view** — expand a thread to see the full conversation
- **Approval queue panel** — see and approve/reject pending agent sends
- **Sync status** — live indicator showing linked inbox sync state
- **Search** — `/` opens search, results replace the message list
- **Attachment handling** — list attachments, open with `xdg-open`
- **Mouse support** — optional, for users who prefer clicking

**Tech stack for the TUI:**
- `ratatui` — the standard Rust TUI framework (used by gitui, lazygit, etc.)
- `crossterm` — terminal backend (cross-platform)
- Ships as part of the same binary: `postbox tui`
- Connects to PostBox API via localhost HTTP (same as any other client)

**Phase:** Phase 6 or later — after the API, SDK, MCP, and dashboard are solid.
The TUI is a power-user delight, not a launch requirement. But it's the kind of
feature that gets a project on the front page of r/unixporn and Hacker News.

---

## 4. Steal: Outbound Guard (from AgenticMail)

**What:** Scan every outgoing email for accidentally leaked secrets before sending.

**Their implementation:** Regex patterns matching API key formats
(AWS `AKIA...`, OpenAI `sk-...`, GitHub `ghp_...`), passwords,
private keys, PII patterns (SSN, credit card numbers), and internal URLs.

**Our implementation should be:**
- Built into the send pipeline, runs before permission check
- Configurable rule sets (enable/disable categories)
- Default-on for all inbox types
- When triggered: block the send, log in audit trail, notify via
  the approval flow (same as permission-based approvals)
- Don't over-engineer — their regex approach is the right level of
  sophistication for v1. No ML, no NLP, just pattern matching.

**Default patterns to ship with:**

```
# API Keys
AWS:        AKIA[0-9A-Z]{16}
OpenAI:     sk-[a-zA-Z0-9]{48}
Anthropic:  sk-ant-[a-zA-Z0-9-]{80,}
GitHub:     ghp_[a-zA-Z0-9]{36}
Stripe:     sk_live_[a-zA-Z0-9]{24,}
Google:     AIza[0-9A-Za-z\-_]{35}

# Credentials
password/secret/token followed by : or = and a value

# Private keys
-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----

# PII
SSN:        \d{3}-\d{2}-\d{4}
Credit card: \d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}
```

**Phase:** Phase 1 — ship with the first send endpoint. It's cheap to
implement and prevents embarrassing agent leaks from day one.

---

## 5. New: Webhook Templates for Common Agent Frameworks

**What:** Pre-built webhook handler examples for LangChain, CrewAI,
LlamaIndex, Autogen, and vanilla Python/TypeScript.

**Why:** AgentMail's docs have quickstart examples for each framework.
We should too. The difference between "supports webhooks" and "here's a
working 20-line LangChain integration" is adoption.

**Ship as:**
- `examples/langchain/` — LangChain tool that wraps PostBox API
- `examples/crewai/` — CrewAI agent with PostBox email capability
- `examples/autogen/` — Autogen function calling integration
- `examples/vanilla-python/` — Plain requests-based example
- `examples/vanilla-typescript/` — Plain fetch-based example
- `examples/claude-code/` — MCP config + usage patterns

**Phase:** Phase 2 alongside the Python SDK.

---

## 6. New: Email Digest Mode

**What:** A periodic summary of inbox activity, useful for agents that
don't need real-time email processing but want a daily/hourly briefing.

**How:**
```
GET /api/v1/inboxes/:id/digest?since=24h
```

Returns a structured summary:
```json
{
  "period": "24h",
  "total_received": 12,
  "total_sent": 3,
  "unread": 5,
  "threads_updated": 4,
  "top_senders": ["john@acme.com (3)", "sarah@client.co (2)"],
  "subjects": ["Re: Q3 Proposal", "Meeting Tuesday", ...],
  "requires_attention": [
    { "id": "...", "reason": "pending_approval", "subject": "..." },
    { "id": "...", "reason": "high_priority_sender", "subject": "..." }
  ]
}
```

**Why:** Not every agent needs to process every email in real-time.
A research agent might check email once a day. A monitoring agent might
want hourly digests. This is a lightweight alternative to webhooks/websockets.

AgenticMail has a `/mail/digest` endpoint that returns inbox with body
previews, but it's just a richer list view. Ours should be an actual
analytical summary.

**Phase:** Phase 2.

---

## 7. New: Notification Providers for Approvals

**What:** When an agent's send is pending approval, notify the human via
their preferred channel, not just the web dashboard.

**Providers:**
- **ntfy.sh** — self-hosted push notifications (free, no app needed)
- **Pushover** — mobile push notifications
- **Telegram** — bot message to your personal chat
- **Email** — send approval notification to your personal email
- **Webhook** — POST to any URL (for custom integrations)
- **Desktop** — libnotify / notify-send on Linux (for local PostBox)

**Config:**
```toml
[notifications.approval]
provider = "ntfy"
topic = "postbox-approvals"

# Or multiple:
# providers = ["ntfy", "email"]
```

**Why:** If you're not staring at the dashboard, you need to know when an
agent wants to send something. AgenticMail emails the owner — we should
support more channels, especially ntfy which fits the self-hosted ethos.

**Phase:** Phase 4 alongside the permission engine.

---

## 8. New: Agent Identity Cards

**What:** Each agent inbox gets a structured identity that other systems
can query — like a vCard but for AI agents.

```json
GET /api/v1/inboxes/:id/identity

{
  "name": "Sales Agent",
  "email": "sales@yourdomain.com",
  "type": "ai-agent",
  "capabilities": ["email", "search", "draft"],
  "permissions": {
    "can_send": true,
    "send_mode": "approval",
    "trust_level": "approval"
  },
  "owner": "nikhil@yourdomain.com",
  "created_at": "2026-03-10T12:00:00Z",
  "metadata": { ... }
}
```

**Why:** As multi-agent systems grow, agents need to discover each other's
capabilities. AgenticMail has an "agent directory" for this. Ours should
be richer — include permission state so agents know whether another agent
can actually send email or is in shadow mode.

**Phase:** Phase 3.

---

## 9. New: Conversation Context Extraction

**What:** Beyond reply extraction (stripping quoted text), provide
structured conversation context from a thread.

```json
GET /api/v1/inboxes/:id/threads/:thread_id/context

{
  "thread_id": "...",
  "subject": "Q3 Proposal",
  "participants": ["john@acme.com", "sales@yourdomain.com"],
  "message_count": 5,
  "summary": [
    { "from": "john", "date": "...", "action": "requested proposal" },
    { "from": "sales-agent", "date": "...", "action": "sent proposal v1" },
    { "from": "john", "date": "...", "action": "asked about pricing on page 3" },
    { "from": "sales-agent", "date": "...", "action": "clarified pricing" },
    { "from": "john", "date": "...", "action": "approved, asked for contract" }
  ],
  "open_questions": ["Contract timeline?"],
  "last_action_by": "john@acme.com",
  "awaiting_response_from": "sales@yourdomain.com"
}
```

**Why:** Agents need thread context to write good replies. Raw message
bodies are noisy. A structured summary of "what happened in this thread"
is far more useful as LLM context than dumping 5 emails into the prompt.

**Implementation:** Can be done with a simple LLM call (optional,
configurable) or heuristic-based extraction (from/to/date/subject patterns).

**Phase:** Phase 3 (intelligence layer).

---

## 10. Revised Phase Plan

Incorporating all improvements:

### Phase 1 — Core + Relay Quick Start (Weeks 3-5)
```
[ ] Native inboxes (Stalwart)
[ ] Gmail relay mode (zero-config quick start)      ← NEW
[ ] Send + receive messages
[ ] Email parsing pipeline (all MIME types)
[ ] Thread assignment
[ ] Reply extraction
[ ] Outbound guard (secret leak scanning)            ← NEW
[ ] Attachments (local filesystem)
[ ] Basic webhook dispatch
[ ] WebSocket events
```

### Phase 2 — Parity + Developer Experience (Weeks 6-8)
```
[ ] Labels, drafts, domains, search
[ ] All webhook event types
[ ] Semantic search (pgvector)
[ ] Interactive CLI shell                             ← NEW
[ ] Email digest endpoint                             ← NEW
[ ] Python SDK
[ ] Framework integration examples                    ← NEW
    (LangChain, CrewAI, Claude Code)
```

### Phase 3 — Linked Inboxes + Intelligence (Weeks 9-12)
```
[ ] Gmail OAuth linked inbox sync
[ ] Generic IMAP linked inbox sync
[ ] Bidirectional sync with conflict resolution
[ ] Agent identity cards                              ← NEW
[ ] Conversation context extraction                   ← NEW
[ ] TypeScript SDK
```

### Phase 4 — Permissions + Safety (Weeks 13-15)
```
[ ] Full permission engine (4 send modes)
[ ] Approval workflows
[ ] Auto-approval rules
[ ] Progressive trust
[ ] Notification providers for approvals              ← NEW
    (ntfy, Pushover, Telegram, email, webhook)
[ ] Audit log + query API
```

### Phase 5 — MCP + Dashboard (Weeks 16-19)
```
[ ] MCP server (stdio + SSE)
[ ] Web dashboard (server-rendered, htmx)
[ ] Dashboard: inbox overview + approval queue
[ ] Dashboard: "Connect Gmail" OAuth flow
[ ] Dashboard: permission toggles
[ ] Hook system (exec hooks)
```

### Phase 6 — TUI + Hardening (Weeks 20-24)
```
[ ] Superfile-style TUI email client                  ← NEW
    (ratatui, multi-panel, vim keys, Nerd Fonts)
[ ] Multi-tenancy
[ ] Rate limiting
[ ] Bounce tracking
[ ] S3 storage backend
[ ] Kubernetes manifests
[ ] Docs site
[ ] v1.0 release
```

### Phase 7+ — Beyond v1.0
```
[ ] SMS/phone number support (Google Voice or Twilio)
[ ] Agent-to-agent messaging (internal, not email)
[ ] Calendar integration (CalDAV via Stalwart)
[ ] postbox-mta (replace Stalwart if needed)
[ ] postbox-imap (replace async-imap)
[ ] Plugin/extension system
```

---

## 11. Ideas Parking Lot

Features worth considering but not yet committed to a phase:

- **Email templates with variable substitution** — agents send
  consistent, branded emails. AgenticMail has this.
- **Scheduled sending** — queue emails for future delivery.
  AgenticMail has this. Low complexity, nice to have.
- **Per-agent email signatures** — auto-appended to outgoing mail.
- **Contact management** — per-inbox address book. Useful for
  agents that interact with recurring contacts.
- **Email rules/filters** — Sieve-style rules for auto-labeling,
  auto-archiving. Pass through to Stalwart's Sieve support.
- **Spam scoring on inbound** — rule-based scoring like AgenticMail's
  spam filter. Stalwart already does spam filtering, so this may be
  redundant. Evaluate after Phase 1.
- **Prometheus/Grafana metrics** — export email volume, latency,
  error rates. Important for production monitoring.
- **DMARC/DKIM aggregate report parsing** — Stalwart generates these.
  Surface them in the dashboard/API.
- **Multi-model embedding support** — let users choose embedding
  model per inbox (different models for different languages/domains).
- **Email-to-webhook bridge** — receive an email, trigger a webhook
  with structured payload. Simpler than full agent integration for
  some use cases.
- **Backup/export** — export all email data as mbox or EML files.
  Important for data portability and compliance.
