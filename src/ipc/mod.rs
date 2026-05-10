//! Inter-process communication between `postbloxd` and its clients
//! (TUI, MCP). One Unix socket, length-prefixed JSON frames.
//!
//! Layers:
//! - [`wire`] — frame codec (u32 BE length + JSON payload)
//! - [`protocol`] — request / response / event JSON shapes
//! - [`hub`] — subscription bus (daemon → many connections)
//! - [`server`] — accept loop + per-connection handler
//! - [`Dispatcher`] — trait the daemon implements to handle ops
//! - [`client`] — small client used by tests, the TUI, and the MCP shim
//!
//! No business logic lives here. Ops are dispatched as opaque
//! `(name, args)` tuples; the daemon decides what they mean.

pub mod client;
pub mod hub;
pub mod protocol;
pub mod server;
pub mod wire;

pub use hub::{Hub, Topic};
pub use protocol::{Event, Request, Response, RpcError};
pub use server::{listen, Dispatcher};

/// Default location of the daemon socket: `$XDG_RUNTIME_DIR/postblox.sock`,
/// falling back to `/tmp` when the var isn't set.
pub fn default_socket_path() -> std::path::PathBuf {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        std::path::PathBuf::from(rt).join("postblox.sock")
    } else {
        std::env::temp_dir().join("postblox.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uses_xdg_runtime_dir_when_set() {
        // SAFETY: tests share a process; isolate via scoped variable swap.
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: single-threaded for this assertion.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        let p = default_socket_path();
        assert_eq!(p, std::path::PathBuf::from("/run/user/1000/postblox.sock"));
        match prev {
            // SAFETY: single-threaded test cleanup; restores prior value.
            Some(v) => unsafe { std::env::set_var("XDG_RUNTIME_DIR", v) },
            // SAFETY: single-threaded test cleanup; clears the variable.
            None => unsafe { std::env::remove_var("XDG_RUNTIME_DIR") },
        }
    }
}
