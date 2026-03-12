default:
    @just --list

check:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test

run:
    cargo run

test:
    cargo test

fmt:
    cargo fmt

build:
    cargo build --release

deny:
    cargo deny check
