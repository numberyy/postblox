# Phase 6 — TUI & v1.0

**Goal:** Superfile-style terminal UI, multi-tenancy, rate limiting,
documentation site, v1.0 release.

**Duration:** 3 weeks

## Prerequisites

- Phase 5 complete (MCP, dashboard, Docker, hooks work)

## Deliverables

### Superfile-Style TUI (`postblox tui`)
- Full ratatui multi-panel terminal UI
- Three-panel layout: inbox list (left), message list (center), preview (right/below)
- Vim keybindings: j/k navigation, Enter to open, r to reply, / to search
- Nerd Font icons, color themes (Nord, Dracula, Catppuccin, Tokyo Night)
- Live updates via WebSocket connection to postblox server
- Inline compose with $EDITOR fallback
- Thread view with conversation history
- Approval queue panel: review + approve/reject inline
- Slop filtering: unslopify toggle, slop score indicators
- Briefing view: daily summary panel
- Attachment handling: download, open with xdg-open
- Mouse support

### Multi-Tenancy
- Multiple organizations on one instance
- Org-scoped data isolation
- Org admin role vs member role

### Rate Limiting
- Per-API-key rate limits
- Configurable in TOML and per-org
- Standard rate limit headers (X-RateLimit-*)

### Bounce Tracking
- Track delivery status: delivered, bounced, complained
- Bounce webhook events
- Auto-disable inbox on repeated hard bounces

### S3 Storage
- Configurable storage backend: local filesystem or S3-compatible
- Attachment storage
- Backup/export support

### Documentation Site
- Static site (mdbook or similar)
- API reference, getting started, configuration, deployment guides
- Published to GitHub Pages

### v1.0 Release
- Semantic versioning: 1.0.0
- Cross-compiled binaries: x86_64/aarch64 Linux (musl) + macOS
- Docker images on ghcr.io
- GitHub Release with changelog
- Announcement: r/rust, r/selfhosted, HN

## Checkpoint

Phase 6 is done when:

1. TUI: launch → browse inboxes → read messages → approve sends → slop filter toggle
2. Multi-tenancy: two orgs on one instance, data isolated
3. Rate limiting: exceed limit → 429 response with retry-after header
4. Bounce tracking: hard bounce → inbox flagged
5. S3: attachments stored in MinIO → retrievable
6. Docs site live on GitHub Pages
7. Release binaries downloadable, Docker image pullable
8. `just check` passes
