FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ src/
COPY migrations/ migrations/
COPY templates/ templates/
COPY static/ static/
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/postblox /postblox
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/postblox-mcp /postblox-mcp
EXPOSE 3000
ENTRYPOINT ["/postblox"]
