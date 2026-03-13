# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-03-13

### Added

#### Email Infrastructure (Phase 0–1)
- Project skeleton with Axum server, PostgreSQL connection pool, `/health` endpoint
- TOML config with environment variable fallback
- Inbox CRUD with Stalwart account lifecycle (create/delete wired to inbox)
- Email send and receive via Stalwart SMTP submission (`lettre`)
- MIME parsing (`mail-parser`) and building (`mail-builder`) behind internal types
- Threading engine with `In-Reply-To` / `References` header tracking
- Reply extraction from quoted email bodies
- Webhook CRUD with HMAC-SHA256 signed dispatch, concurrency cap (20)
- Inbound email receiver endpoint

#### Labels, Drafts, Domains (Phase 2)
- Label CRUD with idempotent message tagging (`ON CONFLICT DO NOTHING`)
- Draft CRUD with send — threading and guard applied on send
- Domain management with Stalwart DNS record integration
- Outbound guard — regex-based secret detection on all send paths
- Full-text search via PostgreSQL `tsvector` + GIN index
- Organization bootstrap and API key management
- Daily briefing endpoint with per-inbox message summaries

#### MCP, Semantic Search, Slop Classification (Phase 3)
- MCP server binary (`postblox-mcp`) — stdio JSON-RPC 2.0, 44 tools
- Semantic search via pgvector HNSW index, OpenAI-compatible embedding API
- Slop classifier — pure Rust heuristic engine, 6 signal types, OTP detection, weighted scoring
- IMAP one-shot pull sync with TLS, batch dedup, 500 UID cap
- Linked account CRUD

#### Permissions, Approvals, Trust (Phase 4)
- Permission engine with `SendMode` (AutoApprove / Approval / Disabled) per inbox
- Rule engine — 6 rule types: DomainAllowlist, DomainBlocklist, TimeWindow, KeywordBlocklist, SlopThreshold, DollarAmount
- Approval workflow — list/get/approve/reject/batch with delivery on approve
- Progressive trust — auto-upgrade from Approval to AutoApprove after threshold
- Audit log with paginated org-scoped queries and composite index
- Notification dispatch to ntfy, webhook, and desktop (`notify-send`) providers

#### Hooks, Docker, Dashboard (Phase 5)
- TOML-configured exec hooks — fire-and-forget event hooks + blocking `before_send` (fail-closed)
- Docker multi-stage build (`rust:1.85-alpine` → `scratch`), docker-compose with 4 services
- Dashboard — minijinja + htmx, 18 routes, 9 pages, cookie auth
- WebSocket hub — broadcast per org, connection limit, ping/pong keepalive, live badge notifications
- Dashboard pages: inboxes, inbox detail, message, thread, approvals, briefing, search, settings, analytics

#### TUI, Rate Limiting, Multi-tenancy (Phase 6)
- TUI binary (`postblox-tui`) — ratatui with 4 themes (Nord/Dracula/Catppuccin/Tokyo Night)
- TUI features: vim keybindings, compose with tui-textarea, WebSocket live data, 8 panel components
- Rate limiting — DashMap sliding window (per-minute + per-hour), 429 + `X-RateLimit-*` headers
- Bounce tracking — delivery status table, auto-disable inbox on ≥5 hard bounces in 24h
- Multi-tenancy — `org_members` table, Admin/Member roles, `AdminOrg` extractor, transactional last-admin guards
- Documentation site — 29 mdbook pages, GitHub Actions deploy to Pages
- Shared delivery helper (`deliver_message`) replacing 4 duplicated call sites
- Shared send authorization (`check_send_allowed`) consolidating guard + permission + hooks

### Fixed

- Stalwart client switched from bearer auth to basic auth (was completely broken)
- Stalwart volume path corrected (`/opt/stalwart-mail` → `/opt/stalwart`)
- SMTP submission via `lettre` replacing non-functional HTTP queue endpoint
- Slop scores now passed to rule evaluation (SlopThreshold rules were never firing)
- Thread lookup capped (was unbounded)
- Trust SQL uses parameterized queries (was format interpolation)
- Dashboard XSS prevention via `escape_html`
- Dashboard returns 400 for validation errors (was 500)
- WebSocket subscribe uses single write lock (was TOCTOU)
- `truncate()` handles multibyte UTF-8 safely
- `PostbloxClient::new` returns `Result` instead of panicking
- API key collision in tests fixed with UUID-based prefixes
- ~540 lines of dead code removed across all phases

[1.0.0]: https://github.com/numbery/postblox/releases/tag/v1.0.0
