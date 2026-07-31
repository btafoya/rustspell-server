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
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rustspell-server /app/rustspell-server
COPY --from=builder /app/openapi.json /app/openapi.json

# Both under /data so a single mounted volume persists dictionaries *and* the
# key/tenant store across restarts. Without RUSTSPELL_DB_PATH set explicitly,
# it would default to an OS data dir inside the container's writable layer —
# ephemeral, meaning every tenant and key would be lost on container restart.
ENV RUSTSPELL_DICTIONARY_DIR=/data/dictionaries
ENV RUSTSPELL_DB_PATH=/data/rustspell.db

EXPOSE 3000 9090

ENTRYPOINT ["/app/rustspell-server"]
