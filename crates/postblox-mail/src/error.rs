//! Errors surfaced by this crate.
//!
//! Per the project's "thiserror per module" rule (see `AGENTS.md`),
//! the mail layer owns its own error type rather than funnelling into
//! a crate-wide `AppError`. Today the only failure surface is
//! [`MailError::Parse`] from [`crate::parser::parse`]; builder /
//! threading / reply paths are infallible by construction.

use thiserror::Error;

/// Errors produced by [`crate::parser`] and other mail-layer routines.
#[derive(Debug, Error)]
pub enum MailError {
    /// The raw bytes could not be parsed as an RFC 5322 message.
    #[error("failed to parse email: {0}")]
    Parse(String),
}
