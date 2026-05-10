//! Common imports for postblox modules.
//!
//! `use postblox::prelude::*;` brings in the entity ID newtypes, the IPC
//! protocol surface (`Hub`, `Topic`, `RpcError`, `Event`), and `DbError`.
//! Modules that need a wider surface area still import directly from
//! [`crate::models`] / [`crate::ipc`] / [`crate::db`]; this module is for
//! the symbols every layer touches.
//!
//! It is intentionally tiny — only types that appear in three or more
//! modules across the crate qualify (per the project's anti-bloat rule
//! about abstractions).

pub use crate::db::DbError;
pub use crate::ipc::{Event, Hub, RpcError, Topic};
pub use crate::models::{AccountId, AttachmentId, DraftId, FolderId, MessageId, ThreadId};
