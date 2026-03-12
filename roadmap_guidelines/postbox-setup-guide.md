# PostBox — Project Setup Guide

> Everything needed to properly bootstrap PostBox as an open-source Rust project on GitHub. This document is the single source of truth for repo setup, conventions, CI/CD, community files, and release strategy.

---

## 1. Naming & Identity

**Name:** PostBox  
**Tagline:** Open-source email infrastructure for AI agents.  
**One-liner:** Self-hosted, performance-first email for AI agents. The open-source alternative to AgentMail.

**Accounts to register:**
- GitHub org or repo: `github.com/<your-username>/postbox`
- crates.io: `postbox` (reserve with `cargo publish` once there's a skeleton)
- PyPI: `postbox-sdk` (for Python SDK)
- npm: `@postbox/sdk` (for TypeScript SDK)
- Domain: `postbox.dev` or `getpostbox.dev` or `postboxmail.dev` (check availability)
- Discord server: for community support

**Branding notes:**
- Keep it simple. No logo needed at launch — a text mark or emoji (📬) is fine.
- If you want a logo later, use a simple geometric postbox/mailbox icon. Don't over-invest here early.

---

## 2. Repository Structure

```
postbox/
│
├── .github/
│   ├── ISSUE_TEMPLATE/
│   │   ├── bug_report.yml              # Structured bug report form
│   │   ├── feature_request.yml         # Feature request form
│   │   └── config.yml                  # "blank issue" link + external links
│   ├── PULL_REQUEST_TEMPLATE.md        # PR checklist
│   ├── workflows/
│   │   ├── ci.yml                      # Build + test + lint on every PR
│   │   ├── release.yml                 # Build binaries + publish on tag
│   │   └── security.yml                # cargo-audit on schedule
│   ├── FUNDING.yml                     # GitHub Sponsors config
│   ├── dependabot.yml                  # Automated dependency updates
│   └── CODEOWNERS                      # Who reviews what
│
├── src/
│   ├── main.rs                         # Entry point + CLI (clap)
│   ├── config.rs                       # TOML config loading + validation
│   ├── app.rs                          # AppState struct, server startup
│   ├── api/                            # Axum HTTP handlers
│   │   ├── mod.rs
│   │   ├── router.rs                   # All route definitions in one place
│   │   ├── auth.rs                     # API key extraction + validation middleware
│   │   ├── error.rs                    # Error types → JSON responses
│   │   ├── inboxes.rs
│   │   ├── messages.rs
│   │   ├── threads.rs
│   │   ├── drafts.rs
│   │   ├── labels.rs
│   │   ├── domains.rs
│   │   ├── webhooks.rs
│   │   ├── search.rs
│   │   ├── approvals.rs
│   │   ├── audit.rs
│   │   └── metrics.rs
│   ├── core/                           # Business logic — NO framework imports here
│   │   ├── mod.rs
│   │   ├── inbox.rs
│   │   ├── message.rs
│   │   ├── thread.rs
│   │   ├── draft.rs
│   │   ├── label.rs
│   │   ├── attachment.rs
│   │   ├── domain.rs
│   │   └── search.rs
│   ├── permissions/                    # Permission engine
│   │   ├── mod.rs
│   │   ├── engine.rs
│   │   ├── rules.rs
│   │   ├── trust.rs
│   │   └── types.rs
│   ├── sync/                           # Linked inbox sync engine
│   │   ├── mod.rs
│   │   ├── engine.rs
│   │   ├── imap.rs
│   │   ├── gmail.rs
│   │   ├── reconcile.rs
│   │   └── queue.rs
│   ├── stalwart/                       # Stalwart REST API client
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   ├── accounts.rs
│   │   └── webhooks.rs
│   ├── mail/                           # Email processing wrappers
│   │   ├── mod.rs
│   │   ├── parser.rs                   # Wraps mail-parser
│   │   ├── builder.rs                  # Wraps mail-builder
│   │   ├── reply_extract.rs            # Quote/signature stripping
│   │   └── threading.rs                # In-Reply-To/References logic
│   ├── events/                         # Event dispatch
│   │   ├── mod.rs
│   │   ├── types.rs
│   │   ├── dispatcher.rs
│   │   ├── webhooks.rs
│   │   └── websocket.rs
│   ├── mcp/                            # MCP server
│   │   ├── mod.rs
│   │   ├── server.rs
│   │   ├── tools.rs
│   │   ├── stdio.rs
│   │   └── sse.rs
│   ├── embeddings/                     # Embedding providers (behind trait)
│   │   ├── mod.rs
│   │   ├── provider.rs                 # trait EmbeddingProvider
│   │   ├── ollama.rs
│   │   ├── openai.rs
│   │   └── noop.rs
│   ├── storage/                        # File storage backends (behind trait)
│   │   ├── mod.rs
│   │   ├── backend.rs                  # trait StorageBackend
│   │   ├── local.rs
│   │   └── s3.rs
│   ├── dashboard/                      # Web UI (server-rendered)
│   │   ├── mod.rs
│   │   ├── routes.rs
│   │   ├── templates/
│   │   └── static/
│   ├── db/                             # Database layer
│   │   ├── mod.rs
│   │   └── pool.rs
│   └── audit/                          # Audit logging
│       ├── mod.rs
│       └── logger.rs
│
├── migrations/                         # SQLx migrations (sequential numbered)
│   └── 20260310000000_initial.sql
│
├── tests/                              # Integration tests
│   ├── api/
│   ├── sync/
│   ├── permissions/
│   ├── common/mod.rs                   # Shared test helpers
│   └── fixtures/                       # Sample .eml files, test data
│
├── sdk/                                # Client SDKs (separate packages)
│   ├── python/
│   │   ├── pyproject.toml
│   │   ├── src/postbox_sdk/
│   │   └── tests/
│   ├── typescript/
│   │   ├── package.json
│   │   ├── src/
│   │   └── tests/
│   └── rust/                           # Rust client crate (dogfood)
│       ├── Cargo.toml
│       └── src/
│
├── deploy/                             # Deployment configs
│   ├── postbox.service                 # systemd unit file
│   ├── Caddyfile.example
│   └── nginx.conf.example
│
├── docs/                               # Documentation (mdbook or plain md)
│   ├── api.md
│   ├── configuration.md
│   ├── deployment.md
│   ├── linked-inboxes.md
│   ├── permissions.md
│   ├── mcp.md
│   └── sdks.md
│
├── Cargo.toml                          # Workspace root / single crate
├── Cargo.lock                          # COMMITTED for binary projects
├── Dockerfile
├── docker-compose.yml                  # PostBox + Stalwart + Postgres + Ollama
├── docker-compose.dev.yml              # Dev: just Postgres + Stalwart
├── postbox.toml.example                # Annotated example config
├── .gitignore
├── .rustfmt.toml                       # Formatting config
├── clippy.toml                         # Clippy config
├── deny.toml                           # cargo-deny license + advisory checks
├── rust-toolchain.toml                 # Pin Rust version
├── LICENSE                             # MIT
├── README.md
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md                  # Contributor Covenant v2.1
├── CHANGELOG.md                        # Keep-a-changelog format
├── ROADMAP.md                          # Links to spec doc phases
├── SECURITY.md                         # Vulnerability reporting process
└── ARCHITECTURE.md                     # High-level design for new contributors
```

### Key structural decisions

**Single crate, not a workspace (initially).** Workspaces add complexity for contributors. Start with one crate. Split into a workspace (`postbox-core`, `postbox-api`, `postbox-mcp`, etc.) only when compile times get painful or you need to publish sub-crates independently.

**`core/` has zero framework imports.** Business logic is pure Rust — no axum, no sqlx in function signatures. The `api/` layer adapts between HTTP and core. This makes core testable without spinning up a server.

**External services are behind traits.** `trait EmbeddingProvider`, `trait StorageBackend`, `trait SmtpSender`. Implementations live in separate files. Tests use mock implementations. This is where the "replace dependencies later" strategy becomes concrete.

**Commit `Cargo.lock`.** PostBox is a binary, not a library. Locking ensures reproducible builds. This is the official Cargo recommendation for binaries.

---

## 3. GitHub Configuration

### 3.1 Repository settings

- **Default branch:** `main`
- **Branch protection on `main`:**
  - Require PR reviews (1 reviewer minimum)
  - Require status checks to pass (CI workflow)
  - Require branches to be up to date
  - No force pushes
  - No deletions
- **Enable "Automatically delete head branches"** after PR merge
- **Disable "Allow merge commits"** — use squash merges only for clean history
- **Enable Discussions** for community Q&A
- **Enable GitHub Sponsors** (even if not collecting yet — shows intent)
- **Topics/tags:** `rust`, `email`, `ai-agent`, `smtp`, `imap`, `mcp`, `self-hosted`
- **Description:** "📬 Open-source email infrastructure for AI agents. Self-hosted, performance-first, MIT licensed."
- **Website:** link to docs or landing page when ready

### 3.2 Labels

Create these labels for consistent issue management:

```
# Type
bug              #d73a4a    Something isn't working
feature          #a2eeef    New feature or enhancement
docs             #0075ca    Documentation improvements
refactor         #e4e669    Code improvement, no behavior change
test             #bfd4f2    Test improvements
chore            #ededed    Maintenance, dependencies, CI

# Priority
priority:high    #b60205    Must fix / implement soon
priority:medium  #fbca04    Should address
priority:low     #0e8a16    Nice to have

# Effort
good first issue #7057ff    Good for newcomers
help wanted      #008672    Extra attention needed
complex          #5319e7    Requires deep knowledge

# Area
area:api         #c5def5    REST API layer
area:core        #c5def5    Business logic
area:sync        #c5def5    Linked inbox sync
area:permissions #c5def5    Permission engine
area:mcp         #c5def5    MCP server
area:dashboard   #c5def5    Web UI
area:sdk         #c5def5    Client SDKs
area:stalwart    #c5def5    Stalwart integration
area:mail        #c5def5    Email parsing/building

# Status
blocked          #b60205    Waiting on something else
wontfix          #ffffff    Not planned
duplicate        #cfd3d7    Already exists
```

### 3.3 Issue templates

**Bug report (`bug_report.yml`):** YAML form with fields for PostBox version, OS, Rust version, steps to reproduce, expected vs actual behavior, and logs.

**Feature request (`feature_request.yml`):** YAML form with fields for use case description, proposed solution, and alternatives considered.

**Config (`config.yml`):** Add a link to Discussions for questions, and a link to SECURITY.md for vulnerabilities.

### 3.4 PR template

Checklist format:

```markdown
## Description
<!-- What does this PR do? -->

## Type
- [ ] Bug fix
- [ ] Feature
- [ ] Refactor
- [ ] Docs
- [ ] Tests

## Checklist
- [ ] Tests pass (`cargo test`)
- [ ] Lints pass (`cargo clippy -- -D warnings`)
- [ ] Formatted (`cargo fmt`)
- [ ] Documentation updated (if applicable)
- [ ] CHANGELOG.md updated (if user-facing)

## Related Issues
<!-- Fixes #123, Closes #456 -->
```

### 3.5 CODEOWNERS

```
# Default: you review everything initially
*                       @your-username

# Area-specific owners as team grows
# src/sync/             @sync-maintainer
# src/mcp/              @mcp-maintainer
# sdk/python/           @python-sdk-maintainer
```

### 3.6 Dependabot

```yaml
# .github/dependabot.yml
version: 2
updates:
  - package-ecosystem: cargo
    directory: "/"
    schedule:
      interval: weekly
    open-pull-requests-limit: 5
    labels:
      - "chore"
  - package-ecosystem: github-actions
    directory: "/"
    schedule:
      interval: weekly
```

### 3.7 FUNDING.yml

```yaml
# .github/FUNDING.yml
github: your-username
# ko_fi: postbox
# open_collective: postbox
```

---

## 4. CI/CD Workflows

### 4.1 CI (every PR + push to main)

**File: `.github/workflows/ci.yml`**

Jobs:
1. **check** — `cargo check` on stable Rust (fast fail)
2. **test** — `cargo test` on stable Rust with Postgres service container
3. **clippy** — `cargo clippy -- -D warnings`
4. **fmt** — `cargo fmt --all -- --check`
5. **deny** — `cargo deny check` (license + advisory audit)
6. **msrv** — test on minimum supported Rust version (1.75)

Use a matrix for OS: `ubuntu-latest` only for now. Add macOS/Windows when there's demand.

Postgres service container for integration tests:

```yaml
services:
  postgres:
    image: pgvector/pgvector:pg17
    env:
      POSTGRES_DB: postbox_test
      POSTGRES_USER: postbox
      POSTGRES_PASSWORD: test
    ports:
      - 5432:5432
    options: >-
      --health-cmd pg_isready
      --health-interval 10s
      --health-timeout 5s
      --health-retries 5
```

### 4.2 Release (on tag push)

**File: `.github/workflows/release.yml`**

Trigger: push tag matching `v*` (e.g., `v0.1.0`).

Jobs:
1. **build** — cross-compile for `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`
2. **docker** — build and push Docker image to `ghcr.io/<username>/postbox:<version>`
3. **release** — create GitHub Release with binaries attached, changelog from CHANGELOG.md
4. **publish** — `cargo publish` to crates.io (optional, do this when API is stable)

Use `cross` for cross-compilation. Static musl builds for Linux so the binary has zero runtime dependencies.

### 4.3 Security audit (weekly schedule)

**File: `.github/workflows/security.yml`**

```yaml
on:
  schedule:
    - cron: '0 0 * * 1'  # Every Monday
  workflow_dispatch:       # Manual trigger

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: rustsec/audit-check@v2
```

---

## 5. Rust Tooling Configuration

### 5.1 rust-toolchain.toml

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

Pin to `stable`, not a specific version. The MSRV is tested separately in CI.

### 5.2 .rustfmt.toml

```toml
edition = "2021"
max_width = 100
use_small_heuristics = "Max"
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
```

Keep it minimal. The default rustfmt is fine for most things. Only override what genuinely helps readability.

### 5.3 clippy.toml

```toml
msrv = "1.75"
```

In CI, run with `-D warnings` to fail on any lint. In your editor, keep it as warnings so it doesn't block flow.

### 5.4 deny.toml (cargo-deny)

```toml
[advisories]
vulnerability = "deny"
unmaintained = "warn"

[licenses]
allow = [
  "MIT",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Unicode-DFS-2016",
]
unlicensed = "deny"
copyleft = "deny"          # Catches AGPL/GPL crates sneaking into deps

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```

This is critical for the monetization story — `copyleft = "deny"` ensures no GPL/AGPL crate accidentally enters your dependency tree.

---

## 6. Community Files

### 6.1 CODE_OF_CONDUCT.md

Use [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) verbatim. It's the standard. Link your enforcement contact email.

### 6.2 CONTRIBUTING.md

Cover:
- How to report bugs (use issue templates)
- How to suggest features
- Development setup (prerequisites, clone, docker compose up, cargo test)
- Project layout (brief version of Section 2 above)
- Architecture principles (core has no HTTP knowledge, traits for external services)
- PR process (branch from main, squash merge, conventional commits)
- Commit message format (Conventional Commits)
- Code style (rustfmt handles it, clippy for lints)
- Testing expectations (new features need tests, bug fixes need regression tests)

### 6.3 SECURITY.md

```markdown
# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in PostBox, please report it responsibly.

**DO NOT open a public issue.**

Email: security@postbox.dev (or your email)

You will receive an acknowledgment within 48 hours. We will work with you to
understand the issue and coordinate a fix before any public disclosure.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.x     | ✅ Latest only |

## Scope

- PostBox API server
- PostBox MCP server
- PostBox web dashboard
- Client SDKs (Python, TypeScript, Rust)

Out of scope: Stalwart Mail Server (report to Stalwart directly), PostgreSQL.
```

### 6.4 CHANGELOG.md

Follow [Keep a Changelog](https://keepachangelog.com/) format:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Initial project scaffolding
- Configuration loading from postbox.toml
- Database connection and migrations
- Health check endpoints
```

### 6.5 ARCHITECTURE.md

A document for new contributors explaining the high-level design. Cover:
- The three-process architecture (PostBox + Stalwart + Postgres)
- How a request flows through the system (HTTP → api/ → core/ → db/ → response)
- How inbound email flows (Stalwart webhook → mail/parser → core/message → events/dispatcher)
- The trait-based extensibility model
- Where to find things (directory map with one-line descriptions)

This is more important than most projects realize. Contributors need a mental model before they can navigate the codebase.

---

## 7. Versioning Strategy

**Semantic Versioning (SemVer):** `MAJOR.MINOR.PATCH`

- **0.x.y** — pre-1.0, breaking changes allowed on minor bumps
- **1.0.0** — stable API, breaking changes only on major bumps

**Tag format:** `v0.1.0`, `v0.2.0`, etc.

**Release cadence (pre-1.0):**
- Phase completion → minor version bump (v0.1, v0.2, v0.3...)
- Critical bug fix → patch version bump (v0.1.1)

**Crate publishing strategy:**
- Don't publish to crates.io until the API is somewhat stable (v0.2+)
- Reserve the name early with a minimal publish if needed
- SDKs follow their own versioning but align with the server version

---

## 8. Branching Strategy

Keep it simple. **GitHub Flow** (not Git Flow):

- `main` is always deployable
- Feature branches: `feat/inbox-crud`, `fix/thread-assignment`, `docs/api-reference`
- No `develop` branch. No `release/*` branches (until you need them).
- Tag `main` for releases: `git tag v0.1.0 && git push --tags`
- Squash merge PRs for clean history

Branch naming convention:
```
feat/short-description     # New feature
fix/short-description      # Bug fix
docs/short-description     # Documentation
refactor/short-description # Code improvement
test/short-description     # Test additions
chore/short-description    # Maintenance
```

---

## 9. Dependency Management Practices

### Adding new dependencies

Before adding a crate, check:
1. **License** — must be MIT, Apache-2.0, BSD, or ISC. Run `cargo deny check` after adding.
2. **Maintenance** — is it actively maintained? When was the last release?
3. **Dependency count** — does it pull in 50 transitive deps? That's a red flag.
4. **Could we build this ourselves in under a day?** If yes, maybe don't add the dep.
5. **Does it need to go behind a trait?** If it's an external service client, yes.

### Updating dependencies

- Dependabot PRs weekly for minor/patch updates
- Review and merge manually (don't auto-merge)
- Major version bumps get their own PR with testing
- Run `cargo deny check` on every PR (CI enforces this)

### cargo-deny as a gate

The `deny.toml` config is your license firewall. It runs in CI on every PR. If someone adds a GPL-licensed transitive dependency, CI fails. This is non-negotiable for the monetization story.

---

## 10. Testing Strategy

### Unit tests

- Live in the same file as the code (`#[cfg(test)] mod tests`)
- Test `core/` logic without any I/O
- Use `#[test]` for sync, `#[tokio::test]` for async
- Mock external services via traits

### Integration tests

- Live in `tests/` directory
- Spin up a real Postgres (CI service container, or `docker compose` locally)
- Test full request → response cycles through the API
- Use a shared `TestApp` helper that sets up the server + database

### Fixtures

- Sample `.eml` files in `tests/fixtures/` for email parsing tests
- Cover: plain text, HTML, multipart/mixed, multipart/alternative, nested messages, attachments, different character encodings, malformed emails
- These are gold — they catch real-world parsing regressions

### What to test first

Priority order:
1. Email parsing pipeline (this is where the edge cases live)
2. Thread assignment logic
3. API auth middleware
4. Permission engine rules
5. Webhook HMAC signing/verification
6. Sync reconciliation logic

---

## 11. Documentation Strategy

### In-repo docs (`docs/`)

Markdown files for each major topic. These ship with the repo and are always in sync with the code.

### Docs site (later)

Use [mdbook](https://rust-lang.github.io/mdBook/) — it's the Rust ecosystem standard. Generates a static site from the `docs/` markdown files. Deploy to GitHub Pages or docs.postbox.dev.

### API reference

Generate an OpenAPI spec from the Axum routes (using `utoipa` or `aide` crate). This becomes the source of truth for SDK generation and the interactive API docs.

### Code documentation

- All public types and functions get `///` doc comments
- Module-level `//!` docs explain the module's purpose and design decisions
- `cargo doc --no-deps --open` should produce useful output from day one

---

## 12. Docker Setup

### Dockerfile (multi-stage)

```
Stage 1: chef    — cargo-chef to cache dependency builds
Stage 2: build   — compile the release binary (musl for static linking)
Stage 3: runtime — FROM scratch or alpine, copy binary + config
```

Target image size: <20MB. This is achievable with `scratch` base + static musl binary.

### docker-compose.yml (production)

Four services:
1. **postbox** — the PostBox binary
2. **stalwart** — Stalwart Mail Server (pulled from their registry, NOT bundled)
3. **db** — pgvector/pgvector:pg17
4. **ollama** — ollama/ollama (optional, for local embeddings)

### docker-compose.dev.yml (development)

Two services only:
1. **db** — Postgres with pgvector
2. **stalwart** — Stalwart for integration testing

PostBox itself runs via `cargo run` on the host for fast iteration.

---

## 13. Initial Commit Checklist

The first commit should include everything needed for a contributor to clone, build, and run tests. Nothing more.

```
[x] Cargo.toml with dependencies
[x] src/main.rs — starts Axum server, loads config, connects to DB
[x] src/config.rs — loads postbox.toml
[x] src/app.rs — AppState struct with DB pool
[x] src/api/router.rs — GET /health, GET /ready
[x] src/api/error.rs — error type → JSON
[x] src/db/mod.rs — connection pool setup
[x] migrations/20260310000000_initial.sql — organizations + api_keys tables (minimal)
[x] postbox.toml.example — annotated example config
[x] Dockerfile
[x] docker-compose.yml
[x] docker-compose.dev.yml
[x] .github/workflows/ci.yml
[x] .gitignore
[x] .rustfmt.toml
[x] clippy.toml
[x] deny.toml
[x] rust-toolchain.toml
[x] LICENSE (MIT)
[x] README.md
[x] CONTRIBUTING.md
[x] CODE_OF_CONDUCT.md
[x] CHANGELOG.md
[x] SECURITY.md
[x] ARCHITECTURE.md
[x] ROADMAP.md
```

**What NOT to include in the first commit:**
- SDK code (too early)
- Dashboard/web UI (too early)
- MCP server (too early)
- Sync engine (too early)
- Anything that doesn't compile and pass `cargo test`

The first commit should represent a working binary that starts, connects to Postgres, and serves a health check. That's it. Everything else comes in subsequent PRs.

---

## 14. First 5 PRs After Initial Commit

These establish the codebase patterns that all future code follows:

1. **PR #1: API key auth middleware + organizations table**
   Sets the pattern for how auth works. Every subsequent endpoint uses this.

2. **PR #2: Inbox CRUD (create, list, get, delete)**
   Sets the pattern for handler → core → db flow. First real Stalwart API integration.

3. **PR #3: Email parsing pipeline (mail-parser wrapper + tests)**
   Establishes the fixtures directory, parsing tests, and the mail/ module pattern.

4. **PR #4: Send message endpoint**
   First outbound email flow. Introduces mail-builder, Stalwart SMTP submission.

5. **PR #5: Receive message (Stalwart webhook → parse → store)**
   First inbound email flow. Introduces the event system and webhook dispatch.

After these 5 PRs, the core loop works: create inbox, send email, receive reply, store in threads. That's v0.1.

---

## 15. Open Source Launch Strategy

### Pre-launch (before making repo public)

- First 5 PRs merged, basic flow works
- README is polished with working quick-start
- Docker compose actually works end-to-end
- At least one demo GIF or screenshot in the README

### Soft launch

- Make repo public
- Post on: r/rust, r/selfhosted, Rust Discord, Hacker News (Show HN)
- Title angle: "I built an open-source alternative to AgentMail for giving AI agents email inboxes"
- Have the Docker quick-start working flawlessly — first impressions matter

### Community building

- Respond to every issue within 24 hours (even if just "thanks, looking into it")
- Tag good first issues aggressively — at least 5-10 at launch
- Write a "Why PostBox exists" blog post explaining the motivation
- Be honest about what's implemented vs. planned (the roadmap covers this)
- Don't oversell. "v0.1, native inboxes work, linked inboxes coming in v0.3" is better than "full AgentMail alternative!!!" when half the features are TODO.

---

## 16. Summary — What Makes a Well-Set-Up OSS Rust Project

The things that separate a professional open-source project from a code dump:

1. **It builds on first try.** `git clone && docker compose up && cargo test` works.
2. **CI is green.** Every PR is gated on tests, clippy, fmt, and license audit.
3. **The README has a working quick-start.** Not "coming soon." Actually works.
4. **There's an ARCHITECTURE.md.** New contributors can navigate the codebase.
5. **Issues are labeled and triaged.** Good first issues exist. Stale issues are closed.
6. **The commit history is clean.** Squash merges, conventional commits, no "fix fix fix" noise.
7. **Dependencies are audited.** cargo-deny runs in CI. No surprise licenses.
8. **The config file is documented.** `postbox.toml.example` has comments on every field.
9. **The project is honest about its status.** Roadmap is public. What works and what doesn't is clear.
10. **The maintainer is responsive.** First 6 months, your response time to issues/PRs is the project's heartbeat.
