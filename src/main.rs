// postblox — refocus in progress.
//
// Phase R0 cut the legacy server, dashboard, multi-tenant, slop, and
// permission stacks. Phases R1+ build the SQLite-backed daemon, the
// rebuilt TUI, the trimmed MCP bridge, and OAuth/keyring auth.
//
// For now the binary is a placeholder so `cargo check` succeeds while
// the pure-rust mail/ and embeddings/ modules continue to compile and
// be tested.

fn main() {
    eprintln!("postblox is being rebuilt (phase R0 complete). See plans/ for the refocus roadmap.");
    std::process::exit(0);
}
