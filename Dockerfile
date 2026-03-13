FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ src/
COPY migrations/ migrations/
COPY templates/ templates/
COPY static/ static/
RUN cargo build --release

FROM scratch
COPY --from=builder /build/target/release/postblox /postblox
COPY --from=builder /build/target/release/postblox-mcp /postblox-mcp
COPY --from=builder /build/target/release/postblox-tui /postblox-tui
EXPOSE 3000
ENTRYPOINT ["/postblox"]
