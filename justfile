default:
    @just --list

check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

run *ARGS:
    cargo run -- {{ARGS}}

test:
    cargo test

fmt:
    cargo fmt

build:
    cargo build --release

deny:
    cargo deny check
