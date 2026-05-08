//! Folder reconciliation: pull new IMAP messages and write them to
//! the local SQLite store.
//!
//! The reconciler is intentionally framework-free: it takes a `&dyn
//! ImapSync` for the network call so tests can substitute a mock and
//! the production path stays the same shape.

pub mod error;
pub mod manager;
pub mod reconciler;
pub mod state;
pub mod worker;

pub use error::SyncError;
pub use manager::WorkerManager;
pub use reconciler::{reconcile_folder, ReconcileReport};
pub use state::{publish_sync_state, SyncState, SyncStateEvent};
pub use worker::{WorkerConfig, WorkerCredentialResolver};
