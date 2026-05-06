//! Folder reconciliation: pull new IMAP messages and write them to
//! the local SQLite store.
//!
//! The reconciler is intentionally framework-free: it takes a `&dyn
//! ImapSync` for the network call so tests can substitute a mock and
//! the production path stays the same shape.

pub mod error;
pub mod reconciler;

pub use error::SyncError;
pub use reconciler::{reconcile_folder, ReconcileReport};
