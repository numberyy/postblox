# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `just demo` recipe and `postblox-demo-seed` binary that spin up a
  self-contained local daemon, seed accounts/folders/messages/drafts/
  MCP gates/approvals, and launch the TUI — no network, keyring, or
  OAuth required.
- `postbloxd` daemon: Unix-socket IPC, length-prefixed JSON frames,
  per-topic broadcast hub, length- and connection-bounded server.
- `DaemonDispatcher` with read ops (`account.list`, `folder.list`,
  `thread.list`, `message.list_by_folder`, `message.list_by_thread`,
  `message.get`, `search`, `audit.list_recent`) and write ops
  (`account.create`/`delete`, `folder.upsert`, `message.set_flags`,
  `draft.create`/`update`/`delete`).
- SQLite schema (9 tables + FTS5 virtual table + 3 triggers), one
  module per entity, in-memory test pool.
- Pure-Rust MIME parser, reply extractor, threading, builder.

### Removed
- **Removed:** `embeddings` module (dead code; semantic search will be agent / gateway driven via a future read-only SQL tool).

[Unreleased]: https://github.com/numbery/postblox/commits/main
