export DATABASE_URL := env("DATABASE_URL", "postgres://postblox:postblox@localhost:5433/postblox_test")
export STALWART_ADMIN_TOKEN := env("STALWART_ADMIN_TOKEN", "")

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

test-integration:
    cargo test -- --include-ignored

stalwart-password:
    @docker logs postblox-stalwart-1 2>&1 | grep -oP "password '\K[^']+"

fmt:
    cargo fmt

build:
    cargo build --release

deny:
    cargo deny check

dev-up:
    docker compose -f docker-compose.dev.yml up -d --wait

dev-down:
    docker compose -f docker-compose.dev.yml down

dev-reset:
    docker compose -f docker-compose.dev.yml down -v
    just dev-up

docker-build:
    docker build -t postblox:latest .

docker-up:
    docker compose up -d

docker-down:
    docker compose down
