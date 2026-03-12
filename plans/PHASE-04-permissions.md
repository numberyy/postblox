# Phase 4 — Permissions & Audit

**Goal:** Full permission engine with 4 send modes, approval workflows,
progressive trust, notification providers, comprehensive audit log.

**Duration:** 2 weeks

## Prerequisites

- Phase 3 complete (linked inboxes, slop pipeline, semantic search work)

## Deliverables

### Permission Engine
- Four send modes per inbox:
  - `shadow` — agent can read, never send
  - `approval` — every outbound queued for human approval
  - `auto_approve` — sends unless rules block
  - `autonomous` — fully autonomous, human informed after
- Configurable per inbox via API and TOML

### Auto-Approval Rules
- Recipient filters (domain allowlist/blocklist, contact list)
- Time-of-day windows
- Dollar amount detection
- Keyword blocklists
- Slop score threshold on outbound (don't send slop)
- Rules evaluated in order, first match wins

### Progressive Trust
- Agents start at `approval`
- Autonomy unlocks based on: success rate, time in service, user override
- Trust score tracked per inbox
- `GET /api/v1/inboxes/:id/trust` — current trust state

### Approval Workflows
- `GET /api/v1/approvals` — pending approvals
- `POST /api/v1/approvals/:id/approve`
- `POST /api/v1/approvals/:id/reject`
- `POST /api/v1/approvals/batch` — batch approve by confidence threshold
- WebSocket events for new approvals

### Notification Providers
- ntfy.sh (self-hosted push)
- Email notification
- Webhook callback
- Desktop notification via libnotify/notify-send
- Configurable in TOML: multiple providers

### Audit Log
- Every action recorded: send, receive, approve, reject, permission change, trust change
- `GET /api/v1/audit` — paginated audit log
- Filterable by inbox, action type, date range
- Immutable append-only table

## Database Migrations

- `permissions` — id, inbox_id, send_mode, rules (JSONB), updated_at
- `approvals` — id, inbox_id, message_id, status, decided_by, decided_at
- `trust_scores` — id, inbox_id, score, total_sends, approved_count, rejected_count
- `audit_log` — id, org_id, inbox_id, action, actor, details (JSONB), created_at
- `notification_config` — id, org_id, provider, config (JSONB), active

## Checkpoint

Phase 4 is done when:

1. Inbox in `approval` mode → send blocked → appears in approvals queue → approve → email sent
2. Auto-approval rule: known contact + low slop → auto-sent
3. Trust: new inbox starts at approval → after N successful sends → auto-upgraded
4. Notification: approval pending → ntfy push received
5. Audit: every action above has audit log entry
6. `just check` passes
