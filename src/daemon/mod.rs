//! Daemon-side glue used by the `postbloxd` binary.
//!
//! The IPC layer is generic over a [`crate::ipc::Dispatcher`]. This
//! module supplies the concrete dispatcher that maps protocol op
//! strings to `db::*` calls and publishes Hub events on writes.
//!
//! Kept in `lib.rs` so integration tests in `tests/` can exercise the
//! same dispatcher the binary uses.

pub mod dispatcher;

pub use dispatcher::{worker_manager_with_idle_config, DaemonDispatcher, DaemonServices};
