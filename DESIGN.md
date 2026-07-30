# Rust Spell Server — Design Document

> Derived from `REQUIREMENTS.md`. This is the architecture and component design; it does not contain implementation code.

## 1. System Overview

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

## 2. Module Breakdown

| Module | Responsibility |
|--------|----------------|
| `src/main.rs` | Bootstrap: init tracing, config, dictionary manager, metrics server, API router, graceful shutdown. |
| `src/config.rs` | Load and validate environment configuration. |
| `src/error.rs` | Application error type mapped to RFC 7807 Problem Details responses. |
| `src/models.rs` | Serde request/response structs and `validator` constraints. |
| `src/engine.rs` | Thin, thread-safe wrapper around `spellbook::Dictionary`. |
| `src/dictionary.rs` | Download, cache, and refresh Hunspell `.aff`/`.dic` files from LibreOffice `.oxt` archives. |
| `src/handlers.rs` | HTTP handlers for `/health`, `/docs`, `/spellcheck`, `/spellcheck/positions`. |
| `src/middleware.rs` | CORS layer configured from allow-list; optional future middleware. |
| `src/metrics.rs` | Prometheus recorder and mini HTTP server on the metrics port. |
| `src/openapi.rs` | Static OpenAPI 3.0 JSON document and validation helper. |
| `src/swagger.rs` | Swagger UI portal served at `/ui` using `swagger-ui-dist`; `/` redirects there. |
| `benches/spellcheck_bench.rs` | Criterion benchmarks for `/spellcheck` throughput. |

### 2.1 Dependency changes

The existing `Cargo.toml` must be updated:

- **Remove**: `nuspell-sys = "0.1"` (unusable; no published crate).
- **Add**: `spellbook = "0.4.2"` (latest from https://github.com/helix-editor/spellbook/, pure-Rust Hunspell-compatible engine).
- **Add**: `reqwest = { version = "0.12", features = ["rustls-tls"] }` (dictionary download).
- **Add**: `directories = "5"` (default cache directory).
- **Add**: `tempfile = "3"` (atomic dictionary cache writes).
- **Add**: `regex = "1"` (text tokenization fallback).
- **Keep**: `axum`, `tokio`, `tower-http`, `serde`, `validator`, `metrics`, `metrics-exporter-prometheus`, `tracing`, `anyhow`, `thiserror`, `uuid`.

## 3. Application State

```rust
pub struct AppState {
    pub engine: Arc<Engine>,
    pub config: Arc<Config>,
}
```

- `Engine` wraps `spellbook::Dictionary` and exposes `check(&str) -> bool` and `suggest(&str) -> Vec<String>`.
- The dictionary itself is immutable after load, so `Engine` is `Send + Sync` and safe behind `Arc`.

## 4. Configuration (`src/config.rs`)

| Env Var | Type | Default | Description |
|-----------|------|---------|-------------|
| `RUSTSPELL_PORT` | `u16` | `3000` | Public API port. |
| `RUSTSPELL_METRICS_PORT` | `u16` | `9090` | Prometheus scrape port. |
| `RUSTSPELL_LOG_LEVEL` | filter | `info` | `tracing` env-filter directive. |
| `RUSTSPELL_LANGUAGE` | `String` | `en_US` | Dictionary locale. |
| `RUSTSPELL_DICTIONARY_URL` | `String` | LibreOffice dictionaries raw URL | Base URL from which to download `{language}.aff` and `{language}.dic`. |
| `RUSTSPELL_DICTIONARY_DIR` | `PathBuf` | OS data dir | Where to cache extracted `.aff`/`.dic` files. |
| `RUSTSPELL_REFRESH_INTERVAL_HOURS` | `u64` | `24` | Re-download if local files are older than this. |
| `RUSTSPELL_CORS_ORIGINS` | `Vec<String>` | — | Comma-separated allow-list. Required at startup. |

Validation rules:
- `RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` must be different.
- `RUSTSPELL_CORS_ORIGINS` must contain at least one valid origin.

## 5. Dictionary Manager (`src/dictionary.rs`)

### 5.1 Flow

1. Compute target directory: `RUSTSPELL_DICTIONARY_DIR / RUSTSPELL_LANGUAGE /`.
2. Check whether `{language}.aff` and `{language}.dic` files exist and are newer than the refresh interval.
3. If missing or stale:
   - Download `{RUSTSPELL_DICTIONARY_URL}/{language}.aff` and `{RUSTSPELL_DICTIONARY_URL}/{language}.dic` via `reqwest`.
   - Atomically move downloaded files into the cache directory.
4. Return the paths to the two dictionary files.

### 5.2 Refresh policy

Because the upstream repository does not expose a version API, "refresh when upstream changes" is implemented as **time-based staleness**: re-download if local files are older than `RUSTSPELL_REFRESH_INTERVAL_HOURS`.

### 5.3 Failure mode

If download, extraction, or parsing fails, the dictionary manager returns an error. `main()` logs it and exits non-zero (fail-fast).

## 6. Spell-Check Engine (`src/engine.rs`)

```rust
pub struct Engine {
    dict: spellbook::Dictionary,
}

impl Engine {
    pub fn new(aff: &str, dic: &str) -> Result<Self, EngineError>;
    pub fn check(&self, word: &str) -> bool;
    pub fn suggest(&self, word: &str) -> Vec<String>;
    pub fn tokenize(&self, text: &str) -> Vec<Token>; // fallback tokenizer
}
```

### 6.1 Tokenization risk

`spellbook` does **not** expose a public tokenizer. The requirement to use an "engine-native tokenizer" cannot be satisfied directly. Design fallback:

- Implement a simple tokenizer in `engine.rs` using `regex` or `unicode-segmentation`.
- Split on Unicode whitespace.
- Strip surrounding punctuation (`.,;:!?"'()[]{}`).
- Preserve the original token and its byte/char positions for `/spellcheck/positions`.

The fallback is documented in `DESIGN.md` and `REQUIREMENTS.md` is updated to reflect the correction.

## 7. Request/Response Models (`src/models.rs`)

### 7.1 Request

```rust
#[derive(Debug, Deserialize, Validate)]
pub struct SpellCheckRequest {
    #[validate(length(min = 0, max = 10000))]
    pub text: Option<String>,

    #[validate(length(min = 0, max = 1000))]
    pub words: Option<Vec<String>>,
}
```

At least one of `text` or `words` must be present. Validation produces a 400 Problem Details response.

### 7.2 Response for `/spellcheck`

```rust
#[derive(Debug, Serialize)]
pub struct SpellCheckResponse {
    pub results: Vec<TokenResult>,
}

#[derive(Debug, Serialize)]
pub struct TokenResult {
    pub token: String,
    pub valid: bool,
    pub suggestions: Vec<String>,
}
```

One entry per token occurrence, preserving input order.

### 7.3 Response for `/spellcheck/positions`

```rust
#[derive(Debug, Serialize)]
pub struct PositionsResponse {
    pub results: Vec<PositionResult>,
}

#[derive(Debug, Serialize)]
pub struct PositionResult {
    pub token: String,
    pub positions: Vec<usize>, // char indexes into the combined input
    pub suggestions: Vec<String>,
}
```

Only misspelled tokens are returned (or all tokens — decide in implementation; design recommends only misspelled).

## 8. HTTP Handlers (`src/handlers.rs`)

| Handler | Route | Behavior |
|---------|-------|----------|
| `swagger_portal` | `GET /ui` | Serve Swagger UI that loads the spec from `GET /docs`; `GET /` redirects to `/ui`. |
| `health_check` | `GET /health` | Return `{ "status": "ok" }`. |
| `health_verbose` | `GET /health?verbose=true` | Return status plus runtime uptime and request count. |
| `openapi_docs` | `GET /docs` | Return embedded `openapi.json` with `application/json`. |
| `spellcheck` | `POST /spellcheck` | Validate body, tokenize text + words, return per-occurrence results. |
| `spellcheck_positions` | `POST /spellcheck/positions` | Validate body, tokenize, return unique misspelled tokens with positions. |

All handlers receive `State<Arc<AppState>>` via Axum’s state extractor.

## 9. Error Handling (`src/error.rs`)

A single `AppError` enum covering:

- `ValidationError`
- `DictionaryDownloadError`
- `DictionaryParseError`
- `InternalError`

Each variant implements `IntoResponse` producing:

```http
HTTP/1.1 400 Bad Request
Content-Type: application/problem+json

{
  "type": "https://rustspell.example/errors/validation-error",
  "title": "Validation error",
  "status": 400,
  "detail": "Either 'text' or 'words' must be provided"
}
```

## 10. Middleware

Replace the broken custom CORS function in `src/main.rs` with `tower_http::cors::CorsLayer`:

```rust
let cors = CorsLayer::new()
    .allow_methods([Method::GET, Method::POST])
    .allow_headers(Any)
    .allow_origin(config.cors_origins.parse());
```

Also configure `TraceLayer` and request-id propagation via `ServiceBuilder`:

```rust
ServiceBuilder::new()
    .set_x_request_id(MakeRequestUuid::default())
    .propagate_x_request_id()
    .layer(TraceLayer::new_for_http())
    .layer(cors)
    .into_inner()
```

## 11. Observability

### 11.1 Metrics

Use `metrics_exporter_prometheus::PrometheusBuilder::install_recorder()` to obtain a `PrometheusHandle`. Spawn a minimal Axum/Hyper server on the metrics port with a single `/metrics` route returning `handle.render()`.

Key metrics:
- `http_requests_total` (counter, labels: `method`, `path`, `status`)
- `http_request_duration_seconds` (histogram)
- `spellcheck_tokens_total` (counter)
- `dictionary_refresh_total` (counter, labels: `result`)

### 11.2 Logging

`tracing_subscriber::fmt()` with `EnvFilter`. Request ID propagation ensures logs can be correlated.

## 12. Graceful Shutdown

Use Tokio’s cross-platform `ctrl_c` signal handler plus `tokio::signal::unix::signal` on Unix for `SIGTERM`. On Windows, only `ctrl_c` is used.

```rust
let shutdown = async {
    let mut ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(SignalKind::terminate())?;
    // select on ctrl_c and term
};

axum::serve(listener, app)
    .with_graceful_shutdown(shutdown)
    .await?;
```

## 13. Deployment

### 13.1 Dockerfile

Multi-stage build:
1. Builder stage: `rust:1.XX-slim` with build deps.
2. Runtime stage: `debian:bookworm-slim` or `gcr.io/distroless/cc-debian12`.
3. Expose `3000` and `9090`.
4. Set `RUSTSPELL_DICTIONARY_DIR=/data/dictionaries` and mount a volume.

### 13.2 docker-compose.yml

```yaml
services:
  rustspell:
    build: .
    ports:
      - "3000:3000"
      - "9090:9090"
    environment:
      RUSTSPELL_LANGUAGE: en_US
      RUSTSPELL_CORS_ORIGINS: http://localhost:3000
    volumes:
      - dict-cache:/data/dictionaries
volumes:
  dict-cache:
```

## 14. Testing Strategy

| Test Type | Scope |
|-----------|-------|
| Unit | Config parsing, model validation, tokenizer, error mapping. |
| Integration | Full Axum app with in-memory `Engine` (loaded from test fixture `.aff`/`.dic`). |
| OpenAPI validation | Test that `/docs` returns JSON matching a schema snapshot. |
| Benchmark | Criterion bench for `POST /spellcheck` word list. |

Use `tower::ServiceExt::oneshot` for integration tests to avoid binding real ports.

## 15. Implementation Order

1. **Foundation**: update `Cargo.toml`, create `config.rs`, `error.rs`, `models.rs`.
2. **Engine + Dictionary**: create `engine.rs`, `dictionary.rs`, wire into `main.rs`.
3. **Handlers + Middleware**: create `handlers.rs`, replace CORS, add request-id/trace layers.
4. **Metrics + OpenAPI**: create `metrics.rs`, `openapi.rs`, serve static spec.
5. **Tests + Benchmarks**: unit tests, integration tests, `benches/spellcheck_bench.rs`.
6. **Deployment**: Dockerfile, docker-compose.yml, README updates.

## 16. Risks and Notes

- **Tokenizer**: `spellbook` has no public tokenizer; use a project-local fallback tokenizer.
- **Dictionary URLs**: LibreOffice extension URLs are versioned in the filename. A default URL is provided, but operators may need to override it for newer releases.
- **Licensing**: `spellbook` is MPL-2.0. The server is MIT. Ensure license compatibility in distribution if `spellbook` is statically linked (Rust crates are compiled in, so MPL requirements apply to modifications to `spellbook` itself, not to the server code).
- **Performance**: The pure-Rust engine and read-only `Arc` should easily meet p50 < 5 ms for single-word checks at >1,000 req/s.
