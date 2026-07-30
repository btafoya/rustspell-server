# syntax=docker/dockerfile:1

# Builder stage
FROM rust:1-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY benches ./benches
COPY openapi.json ./

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rustspell-server /app/rustspell-server
COPY --from=builder /app/openapi.json /app/openapi.json

ENV RUSTSPELL_DICTIONARY_DIR=/data/dictionaries

EXPOSE 3000 9090

ENTRYPOINT ["/app/rustspell-server"]
