//! Local-first email TUI and MCP bridge for AI agents.
//!
//! `postblox` is a single-user, single-machine email client built around a
//! background daemon (`postbloxd`) that owns the SQLite database, the IMAP
//! IDLE workers, and the SMTP submission path. Two clients talk to it over
//! the same Unix socket: a [`ratatui`]-based TUI and an MCP bridge
//! (`postblox-mcp`) that exposes a JSON-RPC stdio API for AI agents.
//!
//! The crate is built on Tokio, SQLite (with FTS5) via `sqlx`, and
//! length-prefixed JSON frames over a Unix-domain socket. There is no
//! network listener, no HTTP service, and no shared service — the daemon is
//! the only component that touches the database or the upstream IMAP/SMTP
//! servers.
//!
//! # Crate layout
//!
//! - [`mail`] — framework-free MIME parsing, reply extraction, threading,
//!   and message building provided by `postblox-mail` and re-exported as
//!   `postblox::mail`.
//! - [`db`] — SQLite layer split per entity (accounts, folders, threads,
//!   messages, attachments, drafts, MCP state, audit log, search).
//! - [`daemon`] — `DaemonDispatcher` mapping IPC op strings to db calls
//!   and publishing events on the hub.
//! - [`ipc`] — Unix-socket IPC: wire codec, protocol types, hub, server,
//!   and client.
//! - [`mcp`] — MCP bridge: JSON-RPC protocol, tool schemas, approval gates,
//!   and notification forwarding.
//! - [`sync`] — IMAP IDLE worker manager and reconciler, one worker per
//!   account.
//! - [`tui`] — `ratatui` TUI client that talks to the daemon over the
//!   socket.
//! - [`oauth`] — Google OAuth2 auth-URL / code-exchange / refresh and
//!   XOAUTH2 helpers.
//! - [`prelude`] — small re-export module of the most-used IDs, error
//!   types, and IPC surface (`use postblox::prelude::*;`).
//! - [`secrets`] — `SecretStore` trait with OS-keyring (default) and
//!   AES-GCM file backends.
//! - [`imap`] / [`smtp`] — IMAP client and SMTP submission.
//! - [`models`] — domain types shared across modules.
//!
//! # Stability
//!
//! `postblox` is a 0.x crate. The Unix-socket IPC wire format and the MCP
//! stdio JSON-RPC protocol are stabilizing but not yet semver-stable;
//! breaking changes are still possible between minor releases. Entity
//! identifiers are UUIDs serialized as strings.

#![deny(clippy::correctness)]
#![warn(clippy::suspicious, clippy::style, clippy::complexity, clippy::perf)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod attachments;
pub mod auth;
pub mod config;
pub mod daemon;
pub mod db;
pub mod imap;
pub mod ipc;
pub use postblox_mail as mail;
pub mod mcp;
pub mod models;
pub mod oauth;
pub mod prelude;
pub mod secrets;
pub mod smtp;
pub mod sync;
/// `ratatui` TUI client that talks to the daemon over the Unix socket.
pub mod tui;
