# Technical Overview

## Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│                         Rust Spell Server                          │
│                                                                    │
│   ┌──────────────┐    ┌──────────────┐    ┌─────────────────────┐  │
│   │  Config      │───▶│  Dictionary  │───▶│  spellbook::        │  │
│   │  (env vars)  │    │  Manager     │    │  Dictionary (Arc)   │  │
│   └──────────────┘    └──────────────┘    └─────────────────────┘  │
│          │                   │                      │                │
│          ▼                   ▼                      ▼                │
│   ┌────────────────────────────────────────────────────────────┐   │
│   │                       AppState (Arc)                        │   │
│   └────────────────────────────────────────────────────────────┘   │
│          │                    │                     │               │
│          ▼                    ▼                     ▼               │
│   ┌──────────┐          ┌──────────┐         ┌──────────────┐      │
│   │ API Axum │          │ Metrics  │         │  Tracing     │      │
│   │ Server   │          │ Server   │         │  (request-id)│      │
│   │ :3000    │          │ :9090    │         │              │      │
│   └──────────┘          └──────────┘         └──────────────┘      │
└────────────────────────────────────────────────────────────────────┘
```

The server is a single Tokio binary with two TCP listeners: the public API on `RUSTSPELL_PORT` and a Prometheus scrape endpoint on `RUSTSPELL_METRICS_PORT`. The spell-checking engine is shared read-only via `Arc`, so handlers never contend for mutable state.

## Module Layout

| Module | Responsibility |
|--------|----------------|
| `src/main.rs` | Bootstrap: init tracing, config, dictionary manager, metrics server, API router, graceful shutdown. |
| `src/config.rs` | Load and validate environment configuration. |
| `src/error.rs` | Application error type mapped to RFC 7807 Problem Details responses. |
| `src/models.rs` | Serde request/response structs and `validator` constraints. |
| `src/engine.rs` | Thin, thread-safe wrapper around `spellbook::Dictionary`. |
| `src/dictionary.rs` | Download, cache, and refresh Hunspell `.aff`/`.dic` files. |
| `src/handlers.rs` | HTTP handlers for `/health`, `/docs`, `/spellcheck`, `/spellcheck/positions`. |
| `src/middleware.rs` | CORS layer configured from allow-list. |
| `src/metrics.rs` | Prometheus recorder and mini HTTP server on the metrics port. |
| `src/openapi.rs` | Static OpenAPI 3.0 JSON document and validation helper. |
| `benches/spellcheck_bench.rs` | Criterion benchmarks for `/spellcheck` throughput. |

## Spell-Check Engine

The engine wraps `spellbook::Dictionary` and exposes:

- `check(word: &str) -> bool`
- `suggest(word: &str) -> Vec<String>`
- `tokenize(text: &str) -> Vec<Token>`

Because `spellbook` does not expose a public tokenizer, the project provides a local tokenizer that splits on Unicode whitespace and strips surrounding punctuation while preserving character positions.

## Build Instructions

```bash
# Build release binary
cargo build --release

# Run tests
cargo test

# Run benchmarks
cargo bench

# Check formatting and lints
cargo fmt --check
cargo clippy --all-targets
```

## Developer Notes

- Keep handlers thin; business logic belongs in `src/engine.rs` and `src/dictionary.rs`.
- Maintain `AppState` as `Arc<Engine>` + `Arc<Config>` shared read-only state.
- Use `tower::ServiceExt::oneshot` for integration tests to avoid binding real ports.
- Dictionaries are downloaded once at startup if the cache is missing or stale.
