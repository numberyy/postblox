# Phase 3 — Linked Inboxes & Semantic Search

**Goal:** Connect real Gmail/IMAP accounts, bidirectional sync, slop
classification pipeline, LLM-based scoring, semantic search via pgvector.

**Duration:** 3 weeks

## Prerequisites

- Phase 2 complete (Gmail relay, labels, drafts, outbound guard work)

## Deliverables

### Linked Gmail Inbox (OAuth)
- OAuth 2.0 flow for Gmail
- Per-account Tokio task for IMAP IDLE
- Initial full sync + incremental updates
- Bidirectional: reads + label changes sync both ways
- Conflict resolution: last-write-wins for labels, stale agent drafts flagged

### Generic IMAP Linked Inbox
- IMAP credentials (encrypted at rest)
- Same sync engine, different auth path
- Supports any standard IMAP provider

### Sync State Machine
- States: IDLE → PULLING → RECONCILE → CONFLICT_RESOLVE → PUSH_WRITES → IDLE
- Each state has timeout and error handling
- Per-account sync task with bounded concurrency (max 50)

### Slop Classification Pipeline (rule-based + LLM)
- 6-stage pipeline: Parse → Sender reputation → Rule-based classify → LLM score → Triage decision → Embed + store
- Rule-based: newsletter headers, unsubscribe links, cold email patterns, OTP codes, transactional patterns
- LLM scoring: Ollama integration for ambiguous messages (slop_score 0.3-0.7 range)
- Fields on every message: `slop_score`, `slop_signals`, `category`, `priority`, `triage_status`
- Auto-archive for slop > 0.8, auto-delete for slop > 0.95

### Semantic Search
- pgvector embeddings on messages
- Configurable embedding provider (Ollama default, OpenAI/Anthropic optional)
- `GET /api/v1/search?q=...&semantic=true` with cosine similarity
- Threshold and limit parameters

### Personal Slop Training
- `POST /api/v1/feedback` — user marks message as slop/not-slop
- Sender-based heuristic learning
- Read/archive/delete pattern tracking

### Unslopify View
- `GET /api/v1/messages?unslopify=true`
- Filters to: slop_score < 0.5 OR known contact OR requires_action OR active thread

### TypeScript SDK
- `@postblox/sdk` on npm
- Covers all endpoints through Phase 3

## New Dependencies

- `async-imap` — IMAP client (Tier 2, behind trait, eventual replacement candidate)
- `oauth2` — OAuth 2.0 flows (Tier 2)
- Ollama integration via HTTP (reqwest, already present)

## Database Migrations

- Add to `messages`: slop_score, slop_signals, category, subcategory, priority, triage_status, triage_action, triaged_at, requires_action, embedding (vector)
- `linked_accounts` — id, inbox_id, provider, credentials_encrypted, sync_state, last_sync_at
- `sender_reputation` — id, org_id, sender_email, sender_domain, message_count, slop_count, last_seen
- `slop_feedback` — id, org_id, message_id, feedback (slop/not_slop), created_at

## Checkpoint

Phase 3 is done when:

1. Link Gmail via OAuth → emails sync → new emails appear via IMAP IDLE
2. Link generic IMAP → sync works
3. Inbound message → slop pipeline → score assigned → auto-archived if > 0.8
4. Semantic search: `?q=meeting notes&semantic=true` returns relevant results
5. Feedback loop: mark messages → sender reputation adjusts
6. Unslopify view returns only actionable messages
7. TypeScript SDK works end-to-end
8. `just check` passes
