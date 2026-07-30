# PR: Initial Implementation - Rust Spell Server

## Overview
Introduces a production-ready Rust HTTP server exposing a Hunspell-compatible spell-checking engine via a small, observable REST API.

## Engine
- Replaced the unusable `nuspell-sys` FFI scaffold with the pure-Rust `spellbook = "0.4.2"` Hunspell-compatible engine.

## New Files
- `Cargo.toml` - Dependencies and metadata
- `Cargo.lock` - Dependency lockfile
- `src/lib.rs` - Library module exports
- `src/main.rs` - Application bootstrap, graceful shutdown, metrics server wiring
- `src/config.rs` - Environment configuration loading and validation
- `src/error.rs` - Application error type mapped to RFC 7807 Problem Details
- `src/models.rs` - Serde request/response structs with validator constraints
- `src/engine.rs` - `spellbook::Dictionary` wrapper with local tokenizer
- `src/dictionary.rs` - Download/cache/refresh of `.aff`/`.dic` dictionary files
- `src/handlers.rs` - HTTP handlers and router builder
- `src/middleware.rs` - CORS layer builder
- `src/metrics.rs` - Prometheus recorder and mini metrics server
- `src/openapi.rs` - Static OpenAPI spec helper
- `openapi.json` - OpenAPI 3.0 document
- `benches/spellcheck_bench.rs` - Criterion throughput benchmark
- `tests/integration.rs` - Axum/Tower integration tests
- `Dockerfile` - Multi-stage container build
- `docker-compose.yml` - Compose service definition
- `.gitignore` - Ignore build artifacts and cached dictionaries

## API Endpoints
| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | None | Liveness/health check |
| GET | `/health?verbose=true` | None | Health check with uptime and request count |
| GET | `/docs` | None | OpenAPI 3.0 specification |
| POST | `/spellcheck` | None | Spell-check text and/or word list |
| POST | `/spellcheck/positions` | None | Misspelled tokens with char positions |

## Testing
- Unit tests for config, error mapping, models, tokenizer, engine, dictionary manager, handlers
- Integration tests for all public endpoints, CORS, and OpenAPI
- Criterion benchmark for `POST /spellcheck` word-list throughput

## Performance Targets
- p50 < 5 ms for `/spellcheck`
- p95 < 50 ms for read operations
- > 1,000 req/s sustained

## Deployment
- Multi-stage `Dockerfile` exposing ports 3000 and 9090
- `docker-compose.yml` with dictionary cache volume

## Backwards Compatibility
None (initial implementation)

## Migration Notes
N/A (initial implementation)
