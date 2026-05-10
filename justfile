default:
    @just --list

check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

run *ARGS:
    cargo run -- {{ARGS}}

test:
    cargo test --workspace

fmt:
    cargo fmt --all

build:
    cargo build --workspace --release

deny:
    cargo deny check

mail-deps:
    cargo tree -p postblox-mail --edges normal > /tmp/postblox-mail-tree.txt
    if grep -E '\b(tokio|sqlx|reqwest|ratatui|lettre) v' /tmp/postblox-mail-tree.txt; then cat /tmp/postblox-mail-tree.txt; exit 1; fi
