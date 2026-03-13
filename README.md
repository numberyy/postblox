# postblox

[![CI](https://github.com/numbery/postblox/actions/workflows/ci.yml/badge.svg)](https://github.com/numbery/postblox/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)

Self-hosted email infrastructure for AI agents.

postblox gives AI agents their own email accounts with send/receive, semantic search, approval workflows, and a 44-tool MCP server — all behind a single REST API. Outbound messages pass through secret detection, slop classification, and configurable rules before delivery.

## Features

### Email
- Send and receive via [Stalwart Mail Server](https://stalw.art) (SMTP/DKIM/SPF/DMARC)
- MIME parsing, threading, reply extraction
- Labels, drafts, domain management
- Full-text search (PostgreSQL tsvector + GIN)
- IMAP sync for linked external accounts

### Security
- Outbound guard — regex-based secret detection on all send paths
- Slop classification — 6-signal heuristic engine with OTP detection
- Approval workflows with batch operations
- Progressive trust — auto-upgrade after N approved sends
- Rule engine — 6 types: domain allowlist/blocklist, time window, keyword blocklist, slop threshold, dollar amount
- Rate limiting (sliding window, per-minute + per-hour)
- Bounce tracking with auto-disable on repeated hard bounces

### AI & MCP
- 44 MCP tools via stdio JSON-RPC — covers the full REST API
- Semantic search via pgvector (HNSW index, OpenAI-compatible embedding API)
- Daily briefing with per-inbox summaries

### Multi-tenancy
- Admin/Member roles with org-scoped data isolation
- API key management, permission engine per inbox
- Audit log with paginated queries

### UI
- **Dashboard**: minijinja + htmx, 18 routes, 9 pages, WebSocket live updates
- **TUI**: ratatui with 4 themes (Nord/Dracula/Catppuccin/Tokyo Night), vim keybindings

### Ops
- Docker scratch image (<20 MB)
- TOML-configured exec hooks (event hooks + blocking `before_send`)
- Notifications: ntfy, webhook, desktop
- Auto-migration on startup

## Quick Start

```bash
git clone https://github.com/numbery/postblox.git
cd postblox
docker compose up -d
```

Create an organization and API key:

```bash
curl -X POST http://localhost:3000/api/v1/organizations \
  -H "Content-Type: application/json" \
  -d '{"name": "my-org"}'
# Returns: { "organization": {...}, "api_key": "pb_..." }
```

Send an email:

```bash
curl -X POST http://localhost:3000/api/v1/inboxes/{inbox_id}/messages \
  -H "Authorization: Bearer pb_YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{"to": ["user@example.com"], "subject": "Hello", "body": "From postblox"}'
```

Open the dashboard at [http://localhost:3000/dashboard](http://localhost:3000/dashboard).

## Architecture

| Module | Purpose |
|--------|---------|
| `src/api/` | HTTP handlers — auth, CRUD, inbound, search, briefing, rate limiting |
| `src/core/` | Business logic — slop classifier, rule engine (pure Rust) |
| `src/db/` | SQL queries via sqlx — one module per entity |
| `src/mail/` | MIME parsing, reply extraction, threading, builder, outbound guard |
| `src/events/` | Webhook dispatch, WebSocket hub, notification + hook dispatch |
| `src/mcp/` | MCP server binary — stdio JSON-RPC, 44 tools |
| `src/dashboard/` | minijinja + htmx — 18 routes, 9 pages, WebSocket live updates |
| `src/tui/` | ratatui TUI — 4 themes, vim keys, WS live data |
| `src/embeddings/` | Semantic search via pgvector, OpenAI-compatible API |
| `src/stalwart/` | Stalwart mail server HTTP + SMTP client |

## Configuration

postblox uses TOML config with environment variable fallback. See the [configuration docs](https://numbery.github.io/postblox/configuration.html) for all options.

```toml
host = "0.0.0.0"
port = 3000
database_url = "postgres://postblox:postblox@localhost:5432/postblox"
stalwart_url = "http://localhost:8080"
stalwart_admin_user = "admin"
```

## MCP Integration

Add to your `.mcp.json`:

```json
{
  "mcpServers": {
    "postblox": {
      "command": "postblox-mcp",
      "env": {
        "POSTBLOX_URL": "http://localhost:3000",
        "POSTBLOX_API_KEY": "pb_YOUR_KEY"
      }
    }
  }
}
```

44 tools covering: inboxes, messages, threads, search, briefing, webhooks, labels, drafts, domains, approvals, permissions, trust, audit, notifications, linked inboxes.

## Stats

| Metric | Value |
|--------|-------|
| Rust files | 99 |
| Lines of code | ~25,500 |
| Tests | 640 |
| MCP tools | 44 |
| Migrations | 27 |
| Doc pages | 29 |

## License

[MIT](LICENSE)
