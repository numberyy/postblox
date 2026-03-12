# Phase 2 — Triage & Tools

**Goal:** Labels, drafts, Gmail relay mode, outbound guard, daily briefing,
interactive CLI shell, Python SDK.

**Duration:** 3 weeks

## Prerequisites

- Phase 1 complete (native inboxes, send/receive, threading work end-to-end)

## Deliverables

### Gmail Relay Mode
- Zero-config quick start using Gmail `+` sub-addressing
- `you+agentname@gmail.com` as inbox address
- PostBox sends via `smtp.gmail.com:587`, receives via IMAP polling
- Three-tier inbox model: Relay (instant) → Native (domain) → Linked (Phase 3)

### Labels & Drafts
- `POST/GET/DELETE /api/v1/inboxes/:id/labels`
- `POST/GET/PUT/DELETE /api/v1/inboxes/:id/drafts`
- `POST /api/v1/inboxes/:id/drafts/:draft_id/send`
- Labels on messages: add, remove, list

### Domain Management
- `POST/GET/DELETE /api/v1/domains`
- DNS verification (MX, SPF, DKIM, DMARC)
- Domain status tracking

### Outbound Guard
- Pre-send secret leak scanning
- Default patterns: AWS keys, OpenAI keys, Anthropic keys, GitHub tokens,
  Stripe keys, private key headers, SSN, credit card numbers
- Blocks + logs + returns error to caller
- Configurable pattern sets in TOML

### Daily Briefing
- `GET /api/v1/briefing?period=24h`
- Structured summary: action items, message counts, top senders
- Rule-based (no LLM yet — that's Phase 3)

### Search
- `GET /api/v1/search?q=...` — full-text search across messages
- PostgreSQL `tsvector` for Phase 2 (pgvector semantic search in Phase 3)

### Interactive CLI Shell
- `postblox shell` binary using ratatui
- REPL commands: `/inbox`, `/read`, `/send`, `/reply`, `/search`
- Connects to running postblox server via REST API

### Python SDK
- `postblox-sdk` on PyPI
- Covers all Phase 1 + Phase 2 endpoints
- Published from `sdk/python/`

### Framework Examples
- `examples/langchain/`, `examples/crewai/`, `examples/vanilla-python/`

## New Dependencies

- `ratatui` — TUI framework for CLI shell
- `lettre` or direct SMTP — Gmail relay sending (Tier 2)

## Checkpoint

Phase 2 is done when:

1. Gmail relay: create relay inbox → send email → receive reply via IMAP poll
2. Labels: create label → apply to message → filter by label
3. Drafts: create → edit → send
4. Outbound guard: message with AWS key pattern → blocked with error
5. Briefing: `GET /briefing` returns structured summary
6. CLI shell: start shell → list inboxes → read message
7. Python SDK: `pip install postblox-sdk` → create inbox → send email
8. `just check` passes
