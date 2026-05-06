# postblox

Local-first email TUI + MCP bridge.

postblox connects to email accounts you already own (Gmail, Fastmail, …),
keeps a local SQLite mirror in real time over IMAP IDLE, and exposes an
MCP bridge so AI agents can read events freely and call tools through
per-tool / per-pattern approval gates.

> Status: undergoing a hard refocus. The legacy v1 (mail server, dashboard,
> multi-tenant orgs, slop classifier) was cut. v2 is being rebuilt phase by
> phase. See the working plan at the top of `CLAUDE.md`.

## Goals (v2)

- **Beautiful, fast TUI** — Gmail-class layout, FTS5 search, live updates.
- **MCP bridge** — outbound notifications ungated; inbound tool calls
  gated per tool + per arg pattern.
- **Instant sync** — IMAP IDLE plus write-through actions.
- **No mail server** — bring your own accounts.
- **Local-first** — SQLite + sqlite-vec, no daemon dependencies beyond
  your terminal.

## Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| R0 | Cut: remove server, dashboard, slop, multi-tenant, postgres | **DONE** |
| R1 | SQLite schema + accounts + folders + threads + messages + FTS5 | next |
| R2 | `postbloxd` daemon — Unix socket IPC, IMAP IDLE workers | |
| R3 | Rewire TUI to socket | |
| R4 | Rebuild MCP — 12 tools + notifications stream + gates | |
| R5 | Auth — OS keyring + OAuth2 (Gmail) | |
| R6 | TUI polish — write-through actions, command bar, themes | |

## License

[MIT](LICENSE)
