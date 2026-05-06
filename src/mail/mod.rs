pub mod builder;
pub mod error;
pub mod parser;
pub mod reply_extract;
pub mod threading;

pub use parser::{Disposition, ParsedAttachment, ParsedEmail};
pub use threading::{ThreadMatch, ThreadRef};
