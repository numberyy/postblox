pub mod builder;
pub mod error;
pub mod parser;
pub mod reply;
pub mod reply_extract;
pub mod threading;

pub use parser::{Disposition, ParsedAttachment, ParsedEmail};
pub use reply::{forward_draft, fwd_prefix, re_prefix, reply_draft, ForwardDraft, ReplyDraft};
pub use threading::{ThreadMatch, ThreadRef};
