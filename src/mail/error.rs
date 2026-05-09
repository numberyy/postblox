//! Errors surfaced by [`crate::mail`].
//!
//! Per the project's "thiserror per module" rule (see `AGENTS.md`),
//! the mail layer owns its own error type rather than funnelling into
//! a crate-wide `AppError`. Today the only failure surface is
//! [`MailError::Parse`] from [`crate::mail::parser::parse`]; builder /
//! threading / reply paths are infallible by construction.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error("failed to parse email: {0}")]
    Parse(String),
}
