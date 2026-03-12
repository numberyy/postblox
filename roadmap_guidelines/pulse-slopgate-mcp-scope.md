# Pulse ↔ SlopGate: Bidirectional MCP Integration Scope

## Overview

Pulse (AI desktop daemon) and SlopGate (message firewall) run as sibling systemd services on the same machine. They communicate bidirectionally — each is both an MCP server and an MCP client to the other.

```
┌─────────────────────┐                    ┌─────────────────────┐
│       Pulse          │   MCP (pull)       │      SlopGate       │
│  (desktop daemon)    │ ─────────────────> │  (message firewall) │
│                      │  briefing, search  │                     │
│  MCP Server:         │ <───────────────── │  MCP Server:        │
│  stdio / Unix socket │  {3 urgent, ...}   │  stdio / SSE        │
│                      │                    │                     │
│                      │   MCP (push/act)   │                     │
│                      │ <═════════════════ │  MCP Client:        │
│                      │  notify, clipboard │  connects to Pulse  │
│                      │ ═════════════════> │                     │
│                      │  {ack}             │                     │
└─────────────────────┘                    └─────────────────────┘
```

Both connections use MCP over Unix domain sockets (lowest latency, no port allocation, no HTTP overhead, both are local Rust services).

---

## Direction 1: Pulse → SlopGate (Pull)

Pulse is the MCP client. SlopGate is the MCP server.

**Use case:** Overlay bar widgets, on-demand queries, user-initiated actions.

### Tools Pulse calls on SlopGate

| Tool | Description | Trigger |
|------|-------------|---------|
| `slopgate.briefing` | Daily summary: action items, slop count, drafts pending | Periodic (every 60s), on Pulse startup, on user hotkey |
| `slopgate.unread` | Unread count by category (personal, work, requires_action) | Overlay bar render loop |
| `slopgate.search` | Semantic search across unified message store | User types in Pulse command palette |
| `slopgate.message.get` | Fetch full message by ID | User clicks notification → expand |
| `slopgate.approvals.list` | Pending outbound approvals (agent drafts waiting for human) | Overlay bar badge count |
| `slopgate.approvals.decide` | Approve/reject a pending send | User acts on approval notification |
| `slopgate.slop.stats` | Slop analytics: today's count, top senders, time saved | Dashboard widget or hotkey |
| `slopgate.triage.override` | Mark a message as not-slop / is-slop (feedback loop) | User corrects a misclassification |

### Example flow

```
[Pulse overlay bar renders every 60s]
  → calls slopgate.unread
  → gets {personal: 2, work: 1, requires_action: 1, slop_blocked: 14}
  → renders: 📬 3 | ⚡1 | 🗑️ 14
```

---

## Direction 2: SlopGate → Pulse (Push)

SlopGate is the MCP client. Pulse is the MCP server.

**Use case:** Real-time notifications, clipboard actions, context injection.

### Tools SlopGate calls on Pulse

| Tool | Description | Trigger |
|------|-------------|---------|
| `pulse.notify` | Show desktop notification (title, body, urgency, actions) | Urgent message arrives, approval needed, daily briefing ready |
| `pulse.clipboard.write` | Write content to clipboard | User requests "copy email body" via approval flow |
| `pulse.context.inject` | Push context into Pulse's awareness (for other agents) | New high-priority message adds to Pulse's active context |
| `pulse.overlay.badge` | Update a specific overlay bar badge/counter | Real-time unread count change without waiting for poll |
| `pulse.sound` | Play notification sound by category | Urgent message from VIP sender |

### Example flow

```
[SlopGate receives email from VIP contact, slop_score: 0.05]
  → calls pulse.notify({
      title: "Email from Aparajita",
      body: "Re: Weekend plans — 2 lines",
      urgency: "high",
      actions: ["read", "reply", "dismiss"]
    })
  → calls pulse.overlay.badge({
      widget: "slopgate",
      requires_action: 2  // was 1, now 2
    })
  → user sees desktop notification immediately
```

---

## Connection Lifecycle

### Startup sequence

1. Both services start via systemd (no ordering dependency required)
2. Each creates its Unix socket: `/run/pulse/mcp.sock`, `/run/slopgate/mcp.sock`
3. Each attempts to connect to the other's socket as a client
4. If the other isn't up yet, retry with backoff (1s, 2s, 4s, max 30s)
5. On connect, exchange MCP `initialize` handshake, discover available tools
6. Both connections are persistent — reconnect on drop

### Graceful degradation

- If Pulse is down: SlopGate continues normally, notifications are lost (acceptable — triage still happens)
- If SlopGate is down: Pulse overlay shows "SlopGate offline" in badge area, no email data
- Neither service crashes if the other is unavailable

### Socket paths (configurable)

```toml
# slopgate.toml
[pulse]
socket = "/run/pulse/mcp.sock"
enabled = true

# pulse.toml
[slopgate]
socket = "/run/slopgate/mcp.sock"
enabled = true
```

---

## Transport: Why Unix Sockets over MCP

MCP supports stdio and HTTP+SSE transports. For two local systemd services:

| | Unix Socket | stdio | HTTP+SSE |
|---|---|---|---|
| Latency | ~10μs | ~10μs | ~100μs |
| Port needed | No | No | Yes |
| Bidirectional | Yes (two sockets) | Awkward (one side must spawn other) | Yes |
| Systemd compatible | Native | Requires process management | Native |
| Multiple clients | Yes | No (1:1) | Yes |

Unix sockets win. Each service opens a socket, the other connects. No spawning, no ports, no HTTP overhead.

**Note:** MCP doesn't officially define a Unix socket transport yet. Implementation options:
- Wrap MCP JSON-RPC over a raw Unix stream (simplest, custom but trivial)
- Use MCP stdio transport with a Unix socket adapter (socat bridge)
- Use HTTP+SSE over a Unix socket (hyper/axum support this natively)

Recommendation: HTTP+SSE over Unix socket via Axum. Gets us standard MCP framing with Unix socket performance. Both services already use Axum.

---

## Notification Schema

SlopGate → Pulse notifications carry enough context for Pulse to render without a callback:

```json
{
  "tool": "pulse.notify",
  "params": {
    "source": "slopgate",
    "event": "message.urgent",
    "title": "Email from john@example.com",
    "body": "Re: Contract review — needs response",
    "urgency": "high",
    "category": "work",
    "message_id": "msg_abc123",
    "actions": [
      {"id": "read", "label": "Read", "callback": "slopgate.message.get"},
      {"id": "dismiss", "label": "Dismiss", "callback": "slopgate.triage.override"},
      {"id": "reply", "label": "Reply", "callback": "slopgate.approvals.create"}
    ],
    "expires_in_seconds": 3600
  }
}
```

Action callbacks reference SlopGate MCP tools. When user clicks "Read", Pulse calls `slopgate.message.get` with the `message_id`. Fully round-trip.

---

## Scope Boundaries

### In scope (Phase 5 — when MCP server ships)
- SlopGate MCP server with tools listed in Direction 1
- Pulse MCP client connecting to SlopGate
- Unix socket transport for both directions
- Basic `pulse.notify` and `pulse.overlay.badge` tools

### In scope (Phase 6+)
- Full bidirectional: SlopGate as MCP client to Pulse
- `pulse.clipboard.write`, `pulse.context.inject`
- Action callbacks in notifications
- Tool discovery / capability negotiation

### Out of scope
- Pulse ↔ SlopGate shared authentication (both are local, trust by socket path)
- Multi-machine deployment (both must be on same host)
- Third-party MCP clients connecting to either (handled separately by each project)

---

## Implementation Notes

- Both projects are Rust + Axum + Tokio — shared ecosystem, shared MCP library potential
- Consider a shared crate (`slopgate-pulse-protocol` or just raw MCP) for the tool schemas
- Pulse's MCP server is also used by Claude Code / other agents — SlopGate is just another client
- SlopGate's MCP server is also used by Claude Code / other agents — Pulse is just another client
- Neither project should have compile-time dependencies on the other
