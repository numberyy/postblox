# Phase 0 — Skeleton

**Goal:** Binary starts, connects to Postgres, serves `/health`, loads config.
Nothing else.

**Duration:** 1-2 days

## Prerequisites

- Rust stable toolchain
- PostgreSQL running locally
- `cargo-deny` installed (`cargo install cargo-deny`)

## Deliverables

- [x] `postblox` binary that starts and listens on a configurable port
- [x] TOML config file loading with env var fallback
- [x] PostgreSQL connection pool (sqlx, max 10 connections)
- [x] `GET /health` returns `{"status": "ok", "database": true}`
- [x] Tracing/logging via `tracing-subscriber` with `RUST_LOG` env filter
- [x] `just check` passes (fmt + clippy + test)

## Files

```
src/main.rs      — entry point, Axum server, health handler
src/config.rs    — Config struct, TOML + env loading
src/db.rs        — PgPool creation
```

## What is NOT in scope

- Inbox CRUD (Phase 1)
- Any email parsing (Phase 1)
- Stalwart integration (Phase 1)
- Authentication (Phase 1)
- Docker (Phase 1)
- SDKs (Phase 2+)
- MCP (Phase 5)

## Checkpoint

Phase 0 is done when:

1. `cargo build` produces a binary
2. `just check` passes (fmt, clippy, test)
3. Binary starts with `DATABASE_URL=... cargo run`
4. `curl localhost:3000/health` returns `{"status":"ok","database":true}`
5. Binary exits cleanly on Ctrl-C
