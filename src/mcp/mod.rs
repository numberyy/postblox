//! MCP bridge for AI agents.
//!
//! Speaks newline-delimited JSON-RPC 2.0 over stdio and forwards the
//! twelve postblox tools to the daemon over the existing IPC socket.

mod gates;
mod protocol;
mod server;
mod tools;

pub use server::{run_stdio, BridgeConfig, BridgeError, DaemonBridge, IpcDaemon, McpBridge};
