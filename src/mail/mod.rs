#![allow(unused)] // Consumed by api/ handlers in Phase 1.

pub mod error;
pub mod parser;
pub mod reply_extract;
pub mod threading;

pub use error::MailError;
pub use parser::ParsedEmail;
pub use threading::{ThreadMatch, ThreadRef};
