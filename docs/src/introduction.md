# postblox

Self-hosted email infrastructure for AI agents.

postblox gives AI agents full email capability — send, receive, search, classify, and manage email — with human oversight built in. It runs as a single Rust binary backed by PostgreSQL and Stalwart Mail Server.

## What it does

- **Inbox management** — create inboxes, send/receive email, automatic threading
- **Permission engine** — four send modes (shadow, approval, auto-approve, autonomous) with configurable rules
- **Slop classification** — 6-signal classifier scores inbound messages for AI-generated content
- **Semantic search** — pgvector-powered similarity search via any OpenAI-compatible embedding API
- **Approval workflows** — human-in-the-loop approval for outbound messages, with progressive trust
- **Webhooks & WebSocket** — real-time event notifications for message received/sent, approvals, trust changes
- **Exec hooks** — TOML-configured shell hooks for event handling and before-send gates
- **MCP server** — 44-tool MCP server for Claude Code and other MCP clients
- **TUI** — terminal UI with 4 themes, vim keybindings, live WebSocket updates
- **Dashboard** — htmx web dashboard with 18 routes

## Binaries

| Binary | Purpose |
|--------|---------|
| `postblox` | Main server (Axum HTTP + WebSocket) |
| `postblox-mcp` | MCP server (stdio JSON-RPC, wraps REST API) |
| `postblox-tui` | Terminal UI client |

## Stack

Rust, Axum, Tokio, PostgreSQL, pgvector, Stalwart Mail Server, minijinja, htmx, ratatui.
