# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **TUI** (ratatui) over the Unix socket: accounts / folders /
  conversations / detail panes, drafts and a full-screen composer, FTS5
  search, live updates from the event bus, attachments, and a `:`-command
  bar. Three curated themes (`dark` default, `light`, `high-contrast`).
- **IMAP IDLE workers + reconciler**: real-time sync per account with
  exponential backoff, OAuth re-auth between cycles, and bounded,
  windowed UID fetches.
- **SMTP submission** per account (implicit TLS / STARTTLS, `AUTH
  PLAIN`/`LOGIN` and `XOAUTH2`).
- **MCP bridge** (`postblox-mcp`): JSON-RPC over stdio, 14 tools,
  per-tool / per-arg-pattern approval gates (fail-closed), and a
  notification stream.
- **Auth**: OS keyring `SecretStore` (AES-GCM file fallback) and Google
  OAuth2 (Gmail), with token refresh.
- `just demo` recipe and `postblox-demo-seed` binary that spin up a
  self-contained local daemon, seed demo data, and launch the TUI — no
  network, keyring, or OAuth required.
- `postbloxd` daemon: Unix-socket IPC, length-prefixed JSON frames,
  per-topic broadcast hub, length- and connection-bounded server.
- SQLite schema (9 tables + FTS5 virtual table + 3 triggers), one
  module per entity, in-memory test pool.
- Pure-Rust MIME parser, reply extractor, threading, builder.

### Changed
- Hardening pass ahead of the public release: bounded the IMAP fetch
  path and added network-phase timeouts; checked port conversions;
  capped in-flight IPC request handlers; redacted approval-notification
  payloads forwarded to MCP clients; refused SMTP credentials over
  unencrypted transports; display-width-aware TUI truncation; a TUI
  panic hook that always restores the terminal; and various smaller
  correctness, security, and contrast fixes.

### Removed
- `embeddings` module (dead code; semantic search will be agent / gateway
  driven via a future read-only SQL tool).
- Dead/unused internal helpers across the db, secrets, IMAP, and TUI
  layers, and the unused root `mail-parser` dependency.

[Unreleased]: https://github.com/numberyy/postblox/commits/main
