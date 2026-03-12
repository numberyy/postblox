# Phase 5 — MCP & Dashboard

**Goal:** MCP server for external AI agent control, web dashboard for
non-technical users, Pulse integration hooks, hook system.

**Duration:** 3 weeks

## Prerequisites

- Phase 4 complete (permissions, approvals, audit log work)

## Deliverables

### MCP Server
- stdio + SSE transports
- Tools: postblox_create_inbox, postblox_list_inboxes, postblox_send_email,
  postblox_list_messages, postblox_get_message, postblox_list_threads,
  postblox_get_thread, postblox_search, postblox_reply, postblox_add_label,
  postblox_create_draft, postblox_send_draft, postblox_briefing,
  postblox_approvals_list, postblox_approvals_decide
- Permission engine applies to MCP calls same as REST API
- `postblox-mcp` binary (stdio mode for Claude Code / Cursor)

### Web Dashboard (htmx)
- Lightweight server-rendered UI
- Pages: inbox list, message view, thread view, approval queue,
  briefing view, slop analytics, settings
- htmx for interactivity (no JS framework)
- Served from the main postblox binary at `/dashboard`

### Pulse Integration Hooks
- SlopGate → Pulse push events: message.requires_action, daily_briefing
- Pulse → SlopGate pull: briefing, unread count, approvals list
- Unix domain socket transport (HTTP+SSE over UDS via Axum)
- Graceful degradation: Pulse down → events buffered briefly then dropped

### Hook System
- Exec hooks on events: message_received, before_send, after_send,
  sync_complete, approval_requested
- Configured in TOML
- Hook process receives JSON on stdin, returns JSON on stdout
- Timeout: 10s per hook, killed after

### Docker Compose
- Complete `docker-compose.yml`: postblox + stalwart + postgres + ollama (optional)
- Health checks on all services
- Volume mounts for persistence
- `docker compose up` to all healthy < 30s

## New Dependencies

- MCP SDK crate (if mature) or hand-rolled JSON-RPC (Tier 3)
- None for htmx (served as static HTML templates)

## Checkpoint

Phase 5 is done when:

1. `postblox-mcp` binary works with Claude Code: create inbox → send email
2. Dashboard: login → view inboxes → read message → approve pending send
3. Pulse integration: overlay bar shows unread count from postblox
4. Hook: message received → exec hook runs → logs output
5. `docker compose up` → all services healthy → API reachable
6. `just check` passes
