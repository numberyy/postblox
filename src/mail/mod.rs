//! Pure email handling: parse, build, thread, reply, quote-strip.
//!
//! Everything here is framework-free — no `tokio`, no `sqlx`, no
//! `reqwest`. The layer rule in `CLAUDE.md` is enforced by code review:
//! anything that does I/O belongs in `imap/`, `smtp/`, or `daemon/`,
//! not here. That property is what lets the bench harness measure
//! parsing throughput without spinning up a runtime.
//!
//! Submodules:
//! - [`parser`] — [`parser::parse`] turns raw RFC 5322 bytes into
//!   [`ParsedEmail`]. Hot path; see CLAUDE.md perf targets.
//! - [`builder`] — MIME assembly for outgoing messages.
//! - [`threading`] — [`threading::assign_thread`] places a parsed
//!   message into an existing thread or starts a new one.
//! - [`reply`] — pre-filled reply / forward [`ReplyDraft`] /
//!   [`ForwardDraft`] construction.
//! - [`reply_extract`] — strip quoted history + signature from a
//!   reply body.
//! - [`error`] — [`error::MailError`] for parse failures.

pub mod builder;
pub mod error;
pub mod parser;
pub mod reply;
pub mod reply_extract;
pub mod threading;

pub use parser::{Disposition, ParsedAttachment, ParsedEmail};
pub use reply::{forward_draft, fwd_prefix, re_prefix, reply_draft, ForwardDraft, ReplyDraft};
pub use threading::{ThreadMatch, ThreadRef};
