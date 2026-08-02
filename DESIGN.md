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
| `src/main.rs` | Bootstrap: init tracing, config, dictionary manager, metrics server, API router, graceful shutdown; also dispatches to the CLI subcommand when invoked as `reset-platform-key`. |
| `src/config.rs` | Load and validate environment configuration. |
| `src/error.rs` | Application error type mapped to RFC 7807 Problem Details responses. |
| `src/models.rs` | Serde request/response structs and `validator` constraints. |
| `src/engine.rs` | Thin, thread-safe wrapper around `spellbook::Dictionary`, plus `EngineRegistry` (§25): a cache of loaded `Engine`s keyed by language for per-request language overrides. |
| `src/dictionary.rs` | Download, cache, and refresh Hunspell `.aff`/`.dic` files from LibreOffice `.oxt` archives, parameterized by language (not just the configured default). |
| `src/handlers.rs` | HTTP handlers for `/health`, `/docs`, `/languages`, `/dictionaries`, `/spellcheck`, `/spellcheck/positions`, `/api-keys*`, `/tenant*`, `/tenants*` (§22, §27). |
| `src/middleware.rs` | CORS layer; dynamic per-tenant `AllowOrigin::predicate` (§23) replaces the static allow-list. |
| `src/metrics.rs` | Prometheus recorder and mini HTTP server on the metrics port. |
| `src/openapi.rs` | Static OpenAPI 3.0 JSON document and validation helper. |
| `src/swagger.rs` | Swagger UI portal served at `/ui` using `swagger-ui-dist`; `/` redirects there. |
| `src/store.rs` | Pluggable persistence layer (SQLite or PostgreSQL, §20): keys, tenants, and registered origins, plus in-memory read caches for the auth/CORS hot path. Renamed and broadened from the single-tenant design's `keystore.rs` (§17) — see §20 for why. |
| `src/auth.rs` | Axum middleware: `X-API-Key` extraction/validation, role gates (`platform`/`admin`/`standard`), origin binding (§23), quota enforcement (§24), dictionary-admin IP gate with `X-Forwarded-For` resolution (§27.3), and per-IP auth-failure rate limiting. |
| `src/usage.rs` | Usage rollup recorder (§26): in-memory accumulation buffer, latency bucket ladder, percentile interpolation, and the scope/window resolution shared by the four `/usage/*` handlers. All SQL stays in `store.rs`. |
| `src/cli.rs` | Offline bootstrap platform-key reset command (§16): argument parsing, confirmation prompt, output formatting, and secrets-file writing. |
| `benches/spellcheck_bench.rs` | Criterion benchmarks for `/spellcheck` throughput. |

### 2.1 Dependency changes

The existing `Cargo.toml` must be updated:

- **Remove**: `nuspell-sys = "0.1"` (unusable; no published crate).
- **Add**: `spellbook = "0.4.2"` (latest from https://github.com/helix-editor/spellbook/, pure-Rust Hunspell-compatible engine).
- **Add**: `reqwest = { version = "0.12", features = ["rustls-tls"] }` (dictionary download).
- **Add**: `directories = "5"` (default cache directory).
- **Add**: `tempfile = "3"` (atomic dictionary cache writes).
- **Add**: `regex = "1"` (text tokenization fallback).
- **Add**: `sqlx = { version = "0.7", default-features = false, features = ["runtime-tokio-rustls", "any", "sqlite", "postgres"] }` — **supersedes the `rusqlite` choice from the single-tenant auth pass (§17)**. `rusqlite` was fine when SQLite was the only backend; once Postgres became a requirement (F33a), `sqlx::any::AnyPool` — built for exactly this "one query surface, pick the backend at runtime" case — replaced it rather than hand-rolling a `Store` trait with two independent sync/async implementations. Runtime string queries (`sqlx::query`/`query_as`) are used throughout, not the compile-time-checked `query!` macros, to avoid requiring a live DB at build time or a checked-in `.sqlx` query cache.
- **Add**: `sha2 = "0.10"` (hash raw API keys at rest; no existing dependency provides a hash function).
- **Keep**: `axum`, `tokio`, `tower-http`, `serde`, `validator`, `metrics`, `metrics-exporter-prometheus`, `tracing`, `anyhow`, `thiserror`, `uuid`.
- **No new RNG or datetime crate**: key generation reuses the already-present `uuid` v4 feature (two UUIDv4s concatenated give ~244 bits of randomness) instead of adding `rand`; timestamps are stored as Unix-epoch integer seconds instead of adding `chrono`.

## 3. Application State

```rust
pub struct AppState {
    pub engines: Arc<EngineRegistry>,
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub rate_limiter: Arc<RateLimiter>,
}
```

- `Engine` wraps `spellbook::Dictionary` and exposes `check(&str) -> bool` and `suggest(&str) -> Vec<String>`. `EngineRegistry` (§25) wraps a cache of `Arc<Engine>` keyed by language, replacing the single `Arc<Engine>` field now that F44 allows a per-request language override.
- The dictionary itself is immutable after load, so `Engine` is `Send + Sync` and safe behind `Arc`.
- `Store` (§20, renamed/broadened from the single-tenant design's `KeyStore`, §17) owns the SQLite-or-Postgres pool and in-memory read caches for keys, tenants, and registered origins; `RateLimiter` (§18) tracks per-IP auth failures. Both are `Send + Sync` and shared read-mostly, matching the `Engine` pattern.

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
| ~~`RUSTSPELL_CORS_ORIGINS`~~ | — | — | **Removed.** CORS is per-tenant now (§23); there is no global allow-list. |
| `RUSTSPELL_DB_PATH` | `PathBuf` | OS data dir | SQLite file for the store, used when `RUSTSPELL_DB_URL` is unset. |
| `RUSTSPELL_DB_URL` | `String` | unset | PostgreSQL connection string (`postgres://...`). When set, takes precedence over `RUSTSPELL_DB_PATH` and the store runs on Postgres via `sqlx::any` (§20). |
| `RUSTSPELL_AUTH_RATE_LIMIT_MAX` | `u32` | `10` | Auth failures allowed per IP per window before cooldown. |
| `RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS` | `u64` | `60` | Sliding window length for counting auth failures. |
| `RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS` | `u64` | `60` | Duration an IP is held at 429 once the threshold is exceeded. |

Validation rules:
- `RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` must be different.
- `RUSTSPELL_DB_URL`, if set, must parse as a valid `postgres://` URL.

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

If download, extraction, or parsing fails, the dictionary manager returns an error. `main()` logs it and exits non-zero (fail-fast) for the startup-time default language. For a non-default language requested at runtime via F44, the same failure surfaces as a 400 to that request instead — see §25.

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

`EngineRegistry` (§25) owns a `HashMap<language, Arc<Engine>>` on top of this; a single `Engine` instance is no longer assumed to be "the" engine.

## 7. Request/Response Models (`src/models.rs`)

### 7.1 Request

```rust
#[derive(Debug, Deserialize, Validate)]
pub struct SpellCheckRequest {
    #[validate(length(min = 0, max = 10000))]
    pub text: Option<String>,

    #[validate(length(min = 0, max = 1000))]
    pub words: Option<Vec<String>>,

    /// Overrides the calling tenant's default language for this request (F44).
    pub language: Option<String>,
}
```

At least one of `text` or `words` must be present. Validation produces a 400 Problem Details response. If `language` is present but not loadable (download/parse failure — see §25), the handler returns a 400 Problem Details response rather than the `validator` schema check (the failure is only knowable after attempting the load, not from the request shape alone).

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

### 7.4 API Key Management Models

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Platform,
    Admin,
    Standard,
}

#[derive(Debug, Deserialize, Validate)]
pub struct CreateApiKeyRequest {
    #[validate(length(min = 1, max = 100))]
    pub label: String,
    /// Must be `admin` or `standard`; `platform` is only ever bootstrap-created (F22), never via this endpoint.
    pub role: Role,
    /// Unix-epoch seconds; must be in the future if present.
    pub expires_at: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyMetadata {
    pub id: String,
    pub label: String,
    pub role: Role,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub last_used_at: Option<u64>,
    pub revoked_at: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CreatedApiKey {
    #[serde(flatten)]
    pub metadata: ApiKeyMetadata,
    /// Raw key value. Returned exactly once, on creation/rotation only.
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyMetadata>,
}
```

`key_hash` never appears in any of these types — it stays internal to `src/store.rs`.

### 7.5 Tenant & Origin Models

```rust
#[derive(Debug, Deserialize, Validate)]
pub struct CreateTenantRequest {
    #[validate(length(min = 1, max = 200))]
    pub name: String,
    pub language: Option<String>,        // defaults to DEFAULT_LANGUAGE (en_US)
    pub quota_limit: Option<u64>,        // defaults to 0 == unlimited, see §21.4
    pub period_start: Option<u64>,
    pub period_end: Option<u64>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct UpdateTenantRequest {
    pub name: Option<String>,
    pub language: Option<String>,
    pub quota_limit: Option<u64>,
    pub period_start: Option<u64>,
    pub period_end: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct TenantMetadata {
    pub id: String,
    pub name: String,
    pub language: String,
    pub quota_limit: u64,
    pub request_count: u64,
    pub period_start: Option<u64>,
    pub period_end: Option<u64>,
    pub suspended_at: Option<u64>,
    pub created_at: u64,
}

#[derive(Debug, Serialize)]
pub struct CreatedTenant {
    #[serde(flatten)]
    pub tenant: TenantMetadata,
    /// The tenant's first admin key, shown once (mirrors `CreatedApiKey`).
    pub admin_key: CreatedApiKey,
}

#[derive(Debug, Serialize)]
pub struct TenantListResponse {
    pub tenants: Vec<TenantMetadata>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct RegisterOriginRequest {
    /// Must parse as a valid `scheme://host[:port]` origin, no path.
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct OriginMetadata {
    pub id: String,
    pub origin: String,
    pub created_at: u64,
}

#[derive(Debug, Serialize)]
pub struct OriginListResponse {
    pub origins: Vec<OriginMetadata>,
}
```

## 8. HTTP Handlers (`src/handlers.rs`, `src/auth.rs`)

| Handler | Route | Auth | Behavior |
|---------|-------|------|----------|
| `swagger_portal` | `GET /ui` | none | Serve Swagger UI that loads the spec from `GET /docs`; `GET /` redirects to `/ui`. |
| `health_check` | `GET /health` | none | Return `{ "status": "ok" }`. |
| `health_verbose` | `GET /health?verbose=true` | none | Return status plus runtime uptime and request count. |
| `openapi_docs` | `GET /docs` | none | Return embedded `openapi.json` with `application/json`. |
| `spellcheck` | `POST /spellcheck` | admin/standard key, tenant not suspended, under quota | Validate body, resolve language (default or override, §25), tokenize, return per-occurrence results. |
| `spellcheck_positions` | `POST /spellcheck/positions` | admin/standard key, tenant not suspended, under quota | Same gating; return unique misspelled tokens with positions. |
| `create_api_key` | `POST /api-keys` | admin key | Validate body, generate + hash a new raw key scoped to the caller’s tenant, persist, return `CreatedApiKey`. |
| `list_api_keys` | `GET /api-keys` | admin key | Return `ApiKeyListResponse`, scoped to the caller’s tenant, from the in-memory cache. |
| `revoke_api_key` | `DELETE /api-keys/{id}` | admin key | Set `revoked_at` (id must belong to caller’s tenant); 204 on success (idempotent — already-revoked returns 204, unknown/foreign `id` returns 404). |
| `rotate_api_key` | `POST /api-keys/{id}/rotate` | admin key | Generate + hash a new raw value for the existing row; keep `id`/`label`/`role`/`created_at`/`expires_at`; reset `last_used_at` to `None`; return `CreatedApiKey`. |
| `get_own_tenant` | `GET /tenant` | admin/standard key | Return `TenantMetadata` for the caller’s own tenant (F39). |
| `list_own_origins` | `GET /tenant/origins` | admin key | Return `OriginListResponse` for the caller’s tenant. |
| `register_origin` | `POST /tenant/origins` | admin key | Validate + persist a new registered origin for the caller’s tenant; update both origin caches (§23). |
| `revoke_origin` | `DELETE /tenant/origins/{id}` | admin key | Remove one registered origin (id must belong to caller’s tenant); update both origin caches. |
| `create_tenant` | `POST /tenants` | platform key, no `Origin` header (F43a) | Create tenant + first admin key in one write; return `CreatedTenant`. |
| `list_tenants` | `GET /tenants` | platform key, no `Origin` header | Return `TenantListResponse` for all tenants. |
| `get_tenant` | `GET /tenants/{id}` | platform key, no `Origin` header | Return `TenantMetadata` for one tenant. |
| `update_tenant` | `PATCH /tenants/{id}` | platform key, no `Origin` header | Apply `UpdateTenantRequest` fields; used by the billing app to set quota/period/name/language. |
| `suspend_tenant` | `POST /tenants/{id}/suspend` | platform key, no `Origin` header | Set `suspended_at`; all of that tenant’s keys reject at auth time until reactivated. |
| `reactivate_tenant` | `POST /tenants/{id}/reactivate` | platform key, no `Origin` header | Clear `suspended_at`. |

All handlers receive `State<Arc<AppState>>` via Axum’s state extractor. Every "Auth" column entry is enforced by `src/auth.rs` middleware layers (§18, §23, §24), not inside the handler bodies — handlers only see requests that already passed role, origin, and quota checks.

## 9. Error Handling (`src/error.rs`)

A single `AppError` enum covering:

- `ValidationError`
- `DictionaryDownloadError`
- `DictionaryParseError`
- `InternalError`
- `Unauthorized` — missing, invalid, expired, or revoked key → 401.
- `Forbidden` — valid key but insufficient role (e.g. standard key on an admin-only route), a suspended tenant, an `Origin` not registered to the caller's tenant (F43), or a `platform` key request carrying any `Origin` header at all (F43a) → 403.
- `QuotaExceeded` — tenant's `request_count >= quota_limit` → 429, distinct from `RateLimited` (different cause, different remediation: contact billing vs. wait out a cooldown).
- `RateLimited` — IP is in an auth-failure cooldown → 429, with a `Retry-After` header set to the remaining cooldown seconds.
- `NotFound` — unknown `id`, or an `id` that exists but belongs to a different tenant (never leak cross-tenant existence via a 403 instead of 404) → 404.
- `InvalidDateRange` — malformed, inverted, half-supplied, or over-retention `start`/`end` on a `/usage/*` query (§26.6) → 400.

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

Replace the broken custom CORS function in `src/main.rs` with `tower_http::cors::CorsLayer`, using a dynamic predicate instead of a static list (§23 has the full design):

```rust
let store = state.store.clone();
let cors = CorsLayer::new()
    .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PATCH])
    .allow_headers(Any)
    .allow_origin(AllowOrigin::predicate(move |origin, _parts| {
        store.is_registered_origin(origin) // sync, in-memory only — see §23
    }));
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

### 10.1 Authentication middleware

`src/auth.rs` adds several `axum::middleware::from_fn_with_state` layers, applied per-route-group rather than globally:

- `require_active_key` (§18) — on every route below except `/tenants*`: `/spellcheck`, `/spellcheck/positions`, `/api-keys*`, `/tenant`, `/tenant/origins*`.
- `require_platform_key` — on `/tenants*` only: requires `role == Platform`, and rejects (403) any request carrying an `Origin` header at all (F43a).
- `require_admin` — on `/api-keys*` and `/tenant/origins*` (mutating routes only — `GET /tenant` is readable by `standard` too), layered after `require_active_key`; rejects `role == Standard` (403).
- `require_origin_binding` (§23) — on the same routes as `require_active_key` except `/tenants*` (which has its own, stricter F43a rule): if the request carries an `Origin` header, it must be registered to the caller's own tenant, else 403.
- `require_active_tenant` — on `/spellcheck*`, `/tenant`, `/tenant/origins*`, `/api-keys*`: rejects (403) if the caller's tenant is suspended.
- `require_quota` (§24) — on `/spellcheck` and `/spellcheck/positions` only, after `require_active_tenant`: rejects (429) if the tenant is over quota; otherwise increments the in-memory counter.

This requires client IP for rate limiting, so `main.rs` binds with connect info:

```rust
axum::serve(
    api_listener,
    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
)
```

**Known limitation**: behind a reverse proxy (the README already documents Caddy/Nginx setups), `ConnectInfo` sees the proxy's IP, not the real client IP, so all proxied clients share one rate-limit bucket. Trusting `X-Forwarded-For` is a spoofing risk without a trusted-proxy allow-list and is out of scope for this PR; flagged here for a future PR if it becomes a problem.

## 11. Observability

### 11.1 Metrics

Use `metrics_exporter_prometheus::PrometheusBuilder::install_recorder()` to obtain a `PrometheusHandle`. Spawn a minimal Axum/Hyper server on the metrics port with a single `/metrics` route returning `handle.render()`.

Key metrics:
- `http_requests_total` (counter, labels: `method`, `path`, `status`)
- `http_request_duration_seconds` (histogram)
- `spellcheck_tokens_total` (counter)
- `dictionary_refresh_total` (counter, labels: `result`)
- `auth_attempts_total` (counter, labels: `result` = `success` | `invalid_key` | `expired` | `revoked` | `rate_limited` | `origin_rejected` | `tenant_suspended` | `quota_exceeded`)
- `api_keys_active` (gauge)
- `tenants_active` (gauge, excludes suspended)
- `tenant_quota_usage_ratio` (gauge, labels: `tenant_id` — `request_count / quota_limit`; only emitted when `quota_limit > 0`)
- `engine_registry_languages_loaded` (gauge)

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
| Unit | `keystore.rs`: create/list/revoke/rotate against an in-memory SQLite DB (`:memory:`), bootstrap-on-empty logic, expiry rejection. |
| Unit | `auth.rs`: rate limiter window/cooldown transitions using injected clock or `tokio::time::pause()`. |
| Integration | Full Axum app with in-memory `Engine` (loaded from test fixture `.aff`/`.dic`). |
| Integration | Auth: missing/invalid/expired/revoked key → 401; standard key on admin route → 403; failure threshold → 429; admin key CRUD round-trip on `/api-keys`. |
| Unit | `store.rs` against both an in-memory SQLite pool and (feature-gated / `#[ignore]`d if no `TEST_DATABASE_URL`) a real Postgres instance: tenant CRUD, origin CRUD, cross-tenant isolation (tenant A's admin key cannot see/revoke tenant B's keys or origins). |
| Unit | `auth.rs` quota middleware: under/at/over `quota_limit`, `quota_limit == 0` treated as unlimited. |
| Unit | `engine.rs` `EngineRegistry`: cache hit avoids re-download; concurrent first-requests for the same new language don't trigger duplicate downloads (double-checked locking). |
| Integration | Multi-tenancy: platform key creates a tenant + admin key in one call; that admin key can manage its own `/api-keys` and `/tenant/origins` but gets 404 (not 403) on another tenant's key/origin IDs; suspended tenant's keys get 403 on `/spellcheck`; over-quota tenant gets 429. |
| Integration | CORS/origin binding: registered origin gets `Access-Control-Allow-Origin` + request succeeds; unregistered origin gets no CORS header; a valid key replayed with a *different* tenant's registered origin as `Origin` gets 403 server-side even though that origin is "known" to the CORS layer. |
| Integration | Platform key + `Origin` header on any `/tenants*` route → 403 (F43a), even for an origin that's registered to some tenant. |
| OpenAPI validation | Test that `/docs` returns JSON matching a schema snapshot. |
| Benchmark | Criterion bench for `POST /spellcheck` word list. |

Use `tower::ServiceExt::oneshot` for integration tests to avoid binding real ports.

## 15. Implementation Order

1. **Foundation**: update `Cargo.toml`, create `config.rs`, `error.rs`, `models.rs`.
2. **Engine + Dictionary**: create `engine.rs`, `dictionary.rs`, wire into `main.rs`.
3. **Handlers + Middleware**: create `handlers.rs`, replace CORS, add request-id/trace layers.
4. **Metrics + OpenAPI**: create `metrics.rs`, `openapi.rs`, serve static spec.
5. **API Key Auth**: create `store.rs` (schema, bootstrap, CRUD, cache — SQLite path only to start) and `auth.rs` (middleware, rate limiter); wire `X-API-Key` gate onto `/spellcheck*` and `/api-keys*`; update `openapi.json` with the `apiKeyAuth` security scheme and new paths.
6. **Multi-Tenancy**: add `tenants`/`tenant_origins` tables and `sqlx::any` Postgres support to `store.rs` (§20–21); add `platform` role, tenant handlers, dynamic CORS predicate, origin binding, and quota middleware (§22–24); extend `engine.rs` with `EngineRegistry` (§25) and thread `language` through `SpellCheckRequest`; update `openapi.json` with `/tenants*`, `/tenant*` paths.
7. **Tests + Benchmarks**: unit tests, integration tests, `benches/spellcheck_bench.rs`.
8. **Deployment**: Dockerfile, docker-compose.yml, README updates.
9. **Usage Rollup** (§26): add `usage_daily`/`usage_latency` tables and flush/query/purge methods to `store.rs`; create `usage.rs` (buffer, bucket ladder, percentiles); add the `record_usage` and `require_usage_scope` middleware and the four `/usage/*` handlers; add `AppError::InvalidDateRange` plus its `docs/errors/` page; spawn flush and purge tasks in `main.rs` and flush on shutdown; update `openapi.json`.
10. **Bootstrap Key Reset CLI** (§16): add `src/cli.rs`, `Store::open_for_cli`, `Store::reset_bootstrap_platform_key`, early subcommand dispatch in `main.rs`, unit/integration tests, and README usage examples.

## 16. Bootstrap Platform Key Reset CLI (`src/cli.rs`)

### 16.1 Overview

A small offline administrative command invoked as a subcommand of the same server binary. It exists because the bootstrap `platform` key printed at first start is shown exactly once; if it is lost or compromised, the operator needs a host-level escape hatch that does not require redeploying or hand-editing the SQLite/Postgres store.

### 16.2 Command dispatch

`main.rs` inspects `std::env::args()` before initializing tracing, dictionaries, metrics, or HTTP listeners. If the first positional argument is `reset-platform-key`, control passes to `src/cli.rs::run()` and the server bootstrap path is skipped entirely. Any other invocation (including no arguments) runs the API server as before. This keeps the command packaged in the same container image and lets `docker compose run --rm rustspell reset-platform-key` work without changing `docker-compose.yml`, `Dockerfile`, or adding a wrapper script.

### 16.3 CLI surface

```
rustspell-server reset-platform-key [--yes] [--json | --quiet]
```

- `--yes`: skip the interactive confirmation prompt. Required in non-TTY environments; otherwise the command fails with a clear error.
- `--json`: print only `{"platform_key":"<raw value>"}\n` to stdout.
- `--quiet`: print only the raw key value followed by a newline to stdout.
- The flags are mutually exclusive.
- No other positional arguments are accepted; unknown arguments produce a non-zero exit and a usage message on stderr.

Default (human-readable) output mirrors the startup bootstrap message:

```
New bootstrap platform API key (save this now, it will not be shown again):
  <raw value>
```

### 16.4 Lifecycle and side effects

1. Load configuration with the existing `config::load()` so `RUSTSPELL_DB_PATH` / `RUSTSPELL_DB_URL` are honored without new environment variables.
2. Open the store via a new `Store::open_for_cli(config)` method. It connects to SQLite/Postgres, runs schema initialization, and reloads in-memory caches, but it does **not** warm dictionaries, start HTTP/metrics servers, spawn usage tasks, or bootstrap a new platform key.
3. If `--yes` is not set, prompt `This will invalidate the existing bootstrap platform key and issue a new one. Continue? [y/N] ` on stderr and read from stdin. Only `y` or `yes` (case-insensitive) proceeds.
4. Call `Store::reset_bootstrap_platform_key()`:
   - Find all active (not revoked, not expired) `platform`-role keys with label `"bootstrap"`.
   - If exactly one exists, rotate it in place: generate a new raw value, update the `key_hash` column, reset `last_used_at` to `NULL`, and update the in-memory key cache of the CLI process so the old hash is removed and the new hash is inserted. Keep the same `id`, `label`, `role`, `created_at`, `expires_at`, and `revoked_at`.
   - If none exists, create one with `tenant_id = NULL`, `label = "bootstrap"`, `role = Platform`, no expiry.
   - If more than one exists, return an error without mutating any key (prevents ambiguous resets).
5. Print the new raw key using the selected output mode.
6. If `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` is set, write the same JSON shape the startup path writes (`{"platform_key":"..."}`) to that path. A failure to write this file is a hard error (non-zero exit) because the operator explicitly configured the path.
7. Exit 0.

A running server process does **not** need to be restarted. Its `Store::authenticate` hot path checks the in-memory key cache first; on a miss, it queries the database, and when it finds the rotated key it inserts the new hash into the cache and evicts any stale hash for the same key id. The old raw value therefore stops authenticating as soon as the new value is first used.

### 16.5 Store additions

Two new public methods on `Store`:

```rust
impl Store {
    /// Open the store for offline administrative commands.
    /// Same as `Store::open` minus bootstrap-key creation and server runtime setup.
    pub async fn open_for_cli(config: &Config) -> anyhow::Result<Self>;

    /// Reset (rotate-or-create) the single bootstrap platform key.
    /// Idempotent-ish: safe to run when a bootstrap key already exists.
    pub async fn reset_bootstrap_platform_key(&self) -> anyhow::Result<CreatedApiKey>;
}
```

`reset_bootstrap_platform_key` reuses existing internal helpers (`generate_raw_key`, `hash_key`, key-cache update pattern from `rotate_key`) but bypasses the tenant-scoped `rotate_key` check because bootstrap keys have `tenant_id = NULL`.

### 16.6 Error behavior

- Any I/O, SQL, or ambiguous-key error produces a non-zero exit code and a descriptive message on stderr.
- In human-readable mode, the new key is never printed if the reset fails.
- `--json`/`--quiet` still emit nothing to stdout on failure; all diagnostics go to stderr.

### 16.7 Testing

| Type | Test |
|------|------|
| Unit | `store.rs`: `reset_bootstrap_platform_key` creates a key on empty store, rotates a single existing bootstrap key, rejects two active bootstrap keys, and invalidates the old raw value. |
| Unit | `store.rs`: file-backed close/reopen test proves a rotated bootstrap key is loadable after `Store::open_for_cli` (then `Store::open`) reloads it. |
| Unit | `store.rs`: a running `Store` honors a key rotated by a second `Store` on the same database via the DB fallback path, and evicts the stale hash once the new value is used. |
| Unit | `cli.rs`: argument parsing rejects `--json --quiet`, requires `--yes` or TTY, and selects the correct output formatter. |
| Integration | Run the CLI binary subcommand against a temporary SQLite file and verify the printed key authenticates as `platform` on the next server start. |

### 16.8 Documentation

- Update `README.md` with the `docker compose run --rm rustspell reset-platform-key` example.
- No OpenAPI change (there is no HTTP endpoint).

### 16.9 Security notes

- The CLI relies on host/container filesystem access as the authorization proof. Anyone who can run a container with the `data` volume mounted can reset the bootstrap key. This is the same trust boundary as editing `rustspell.db` directly.
- Interactive confirmation and the `--yes` flag follow the destructive-operation convention.
- The raw key is printed exactly once and, if configured, written to the secrets file. It is not logged.

## 17. Risks and Notes

- **Tokenizer**: `spellbook` has no public tokenizer; use a project-local fallback tokenizer.
- **Dictionary URLs**: LibreOffice extension URLs are versioned in the filename. A default URL is provided, but operators may need to override it for newer releases.
- **Licensing**: `spellbook` is MPL-2.0. The server is MIT. Ensure license compatibility in distribution if `spellbook` is statically linked (Rust crates are compiled in, so MPL requirements apply to modifications to `spellbook` itself, not to the server code).
- **Performance**: The pure-Rust engine and read-only `Arc` should easily meet p50 < 5 ms for single-word checks at >1,000 req/s.
- **Auth hot-path performance**: key validation must not hit SQLite per request. `KeyStore` keeps an `RwLock<HashMap<key_hash, KeyRecord>>` in memory, loaded at startup and kept in sync on every mutation; SQLite is only touched on create/rotate/revoke and on the fire-and-forget `last_used_at` update. NF01/NF03 targets were set before auth existed and should be re-benchmarked once this lands.
- **`last_used_at` write volume**: updated on every successful auth via a fire-and-forget `spawn_blocking` write, not batched. // ponytail: per-request write, debounce (e.g. once/60s/key) if profiling shows write pressure at expected traffic.
- **Hashing choice**: keys are hashed with SHA-256, not a slow KDF (bcrypt/argon2). Unlike passwords, these are server-generated ~244-bit random tokens — the threat model is leakage, not brute force, so a fast hash for O(1) lookup is correct and standard practice (GitHub, Stripe do the same for API tokens).
- **Bootstrap re-trigger condition**: "empty key store" (F22) is interpreted as *no non-revoked admin-role key exists*, not *zero rows in the table* — since revocation is soft-delete, the table is never literally empty after first use. Checked via `SELECT COUNT(*) FROM api_keys WHERE role='admin' AND revoked_at IS NULL`.
- **Admin lockout**: if every admin key is revoked while the server is running, there is no way to mint a new one until restart (bootstrap only runs at startup). This is intentional (no unauthenticated escape hatch at runtime) but worth calling out operationally.
- **SQLite concurrency**: `sqlx::SqlitePool` (via `sqlx::any`) with `PRAGMA journal_mode=WAL` set on connection. SQLite still serializes writers regardless of pool size, and all hot-path reads go through the in-memory cache, so the pool mainly buys the same async interface Postgres uses — not extra write throughput.
- **Quota counter durability**: like `last_used_at`, `request_count` is incremented in-memory (source of truth for the hot-path 429 decision) and flushed to the store fire-and-forget. A crash between increment and flush undercounts usage from the billing app's view (a little free quota, never over-counting into a false 429). // ponytail: fire-and-forget per request; batch/debounce if profiling shows write pressure, same ceiling as `last_used_at`.
- **Cross-tenant ID enumeration**: `/api-keys/{id}`, `/tenant/origins/{id}`, and `/tenants/{id}` must all return 404 (not 403) for IDs that exist but belong to a different tenant/scope — otherwise the response code itself leaks that an ID exists, allowing enumeration.
- **Usage rollup row growth**: bounded by F51/F52 — roughly 180k rows in `usage_daily` plus ~100k in `usage_latency` at 100 tenants × 5 languages × 90 days. Single-digit MB. The real cardinality risk is `language`: a tenant sending many per-request `language` overrides (F44) multiplies its row count. Bounded in practice because the value must name a loadable dictionary, but worth watching if the supported-language list grows large.
- **Usage recording layer position**: `record_usage` must stay the innermost `.route_layer()` on `protected_spellcheck`. Moving it outward silently starts recording auth/quota rejections, which breaks the "rollup counts equal billable requests" invariant that `REQUIREMENTS.md` §6 depends on. §26.10's 429-records-nothing test is the guard.
- **Flush interval vs. durability**: up to 10 seconds of usage data is lost on an unclean kill. Consistent with F49 and the existing quota-counter tradeoff, and the graceful-shutdown path flushes explicitly so a normal deploy loses nothing. Only an actual crash or `SIGKILL` costs data.
- **`tenant_quota_usage_ratio{tenant_id=...}` cardinality**: one Prometheus series per tenant. Fine for tens–hundreds of tenants; revisit (aggregate instead, or drop the per-tenant label) if the tenant count grows into the thousands.
- **Cold-language latency**: the first `/spellcheck*` request for a language not yet cached in `EngineRegistry` pays a full download+parse (potentially seconds), unlike the sub-5ms p50 target for already-loaded languages. This is a one-time cost per language per process lifetime, not per-request, but is a real latency spike for whichever request happens to trigger it.
- **Storage backend parity risk**: `sqlx::any` gives one query surface, but SQLite and Postgres still differ in transaction/locking semantics under concurrent writes. The schema (§21.1) deliberately avoids backend-specific types (`BIGINT`/`TEXT` only, no `SERIAL`/`AUTOINCREMENT`) to minimize drift, but integration tests should run against both backends before this ships (see §14).

## 17. API Key Store (`src/keystore.rs`)

### 17.1 Schema

```sql
CREATE TABLE IF NOT EXISTS api_keys (
    id            TEXT PRIMARY KEY,               -- uuid v4
    label         TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'standard')),
    key_hash      TEXT NOT NULL UNIQUE,            -- hex SHA-256 of the raw key
    created_at    INTEGER NOT NULL,                -- unix epoch seconds
    expires_at    INTEGER,                         -- unix epoch seconds, nullable
    last_used_at  INTEGER,                         -- unix epoch seconds, nullable
    revoked_at    INTEGER                          -- unix epoch seconds, nullable
);
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);
```

Applied via `conn.execute_batch` on startup, alongside `PRAGMA journal_mode=WAL;`.

### 17.2 Key generation and hashing

```rust
/// Raw value shown to the caller exactly once: "rsk_" + 2×UUIDv4 (no dashes).
fn generate_raw_key() -> String {
    format!("rsk_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn hash_key(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}
```

(`hex` is already transitively available via existing deps' feature sets or trivially replaced with a manual hex formatter — evaluate at implementation time before adding it as a direct dependency.)

### 17.3 Struct and API

```rust
pub struct KeyStore {
    conn: Mutex<rusqlite::Connection>,
    cache: RwLock<HashMap<String, KeyRecord>>, // keyed by key_hash
}

impl KeyStore {
    /// Opens/creates the DB, runs schema init, loads the cache, and — if no
    /// non-revoked admin key exists — creates + prints one bootstrap admin key.
    pub fn open(path: &Path) -> anyhow::Result<(Self, Option<CreatedApiKey>)>;

    /// Hot path: O(1) cache lookup + expiry/revocation check. No I/O.
    pub fn authenticate(&self, raw_key: &str) -> Option<KeyRecord>;

    /// Fire-and-forget: spawn_blocking a single UPDATE, ignore errors beyond a log line.
    pub fn touch_last_used(&self, id: &str);

    pub async fn create(&self, label: String, role: Role, expires_at: Option<u64>) -> anyhow::Result<CreatedApiKey>;
    pub fn list(&self) -> Vec<ApiKeyMetadata>; // reads cache only
    pub async fn revoke(&self, id: &str) -> anyhow::Result<bool>; // false if id unknown
    pub async fn rotate(&self, id: &str) -> anyhow::Result<Option<CreatedApiKey>>; // None if id unknown
}
```

`create`/`revoke`/`rotate` write SQLite first (via `spawn_blocking` on the shared `Mutex<Connection>`), then update the in-memory cache under the `RwLock` write guard — DB is the source of truth, cache is a derived, always-fresh mirror.

## 18. Authentication Middleware & Rate Limiting (`src/auth.rs`)

```rust
pub struct RateLimiter {
    state: Mutex<HashMap<IpAddr, FailureWindow>>,
    max_failures: u32,
    window: Duration,
    cooldown: Duration,
}

struct FailureWindow {
    failures: Vec<Instant>,
    cooldown_until: Option<Instant>,
}

impl RateLimiter {
    /// Returns Err(remaining_cooldown) if this IP is currently locked out.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration>;

    /// Records a failed auth attempt; may start a new cooldown for this IP.
    pub fn record_failure(&self, ip: IpAddr);
}
```

Request flow for a protected route:

1. `require_active_key` extracts `ConnectInfo<SocketAddr>` and the `X-API-Key` header.
2. `rate_limiter.check(ip)` — if locked out, short-circuit with `AppError::RateLimited` (429) before touching the key store.
3. `keystore.authenticate(raw_key)` — `None` (missing header, unknown hash, revoked, or expired) → `rate_limiter.record_failure(ip)` then `AppError::Unauthorized` (401).
4. On success: `keystore.touch_last_used(&record.id)` (fire-and-forget), insert `record` into `request.extensions_mut()`, call the next layer.
5. `require_admin` (only on `/api-keys*`, runs after step 4) reads the `KeyRecord` from extensions; `Role::Standard` → `AppError::Forbidden` (403).

## 19. OpenAPI Specification Updates

Add a reusable security scheme and reference it on the newly protected paths; unauthenticated paths (`/health`, `/docs`, `/ui`, `/metrics`) are unchanged.

```json
"components": {
  "securitySchemes": {
    "apiKeyAuth": {
      "type": "apiKey",
      "in": "header",
      "name": "X-API-Key"
    }
  }
},
```

```json
"/spellcheck": {
  "post": {
    "security": [{ "apiKeyAuth": [] }],
    "responses": {
      "401": { "$ref": "#/components/responses/Unauthorized" },
      "429": { "$ref": "#/components/responses/RateLimited" }
    }
  }
}
```

New paths to add under `paths`: `POST /api-keys`, `GET /api-keys`, `DELETE /api-keys/{id}`, `POST /api-keys/{id}/rotate` — each with `"security": [{ "apiKeyAuth": [] }]` and a `403` response in addition to `401`/`429`, tagged `"API Keys"`. New schemas to add under `components/schemas`: `Role`, `CreateApiKeyRequest`, `ApiKeyMetadata`, `CreatedApiKey`, `ApiKeyListResponse` (mirroring §7.4 field-for-field). These are additive edits to the existing hand-written `openapi.json`, validated by the existing `spec_is_valid_openapi` test (`src/openapi.rs`).

Multi-tenancy adds further paths under the same `apiKeyAuth` scheme, tagged `"Tenants"` / `"Origins"`: `POST /tenants`, `GET /tenants`, `GET /tenants/{id}`, `PATCH /tenants/{id}`, `POST /tenants/{id}/suspend`, `POST /tenants/{id}/reactivate`, `GET /tenant`, `GET /tenant/origins`, `POST /tenant/origins`, `DELETE /tenant/origins/{id}` — schemas per §7.5. `/spellcheck` and `/spellcheck/positions` request bodies gain the optional `language` field; their `responses` gain `429` with a description distinguishing quota-exceeded from rate-limited (same status code, different `type` URI in the Problem Details body — see §9's `QuotaExceeded` vs `RateLimited`).

---

# Multi-Tenancy (SaaS) Design

The following sections (§20–25) implement `REQUIREMENTS.md` §3.9 (F34–F46, F43a) on top of §17–19 above. They supersede §17's SQLite-only storage choice and §10's static CORS allow-list; everywhere else in this document has already been updated in place to reflect that.

## 20. Storage Abstraction (`src/store.rs`)

### 20.1 Why `sqlx::Any` instead of extending `rusqlite`

§17 chose `rusqlite` because SQLite was the only backend in scope, and a single `Mutex<Connection>` is simpler than a pool for a single-writer embedded DB. Postgres support (F33a) breaks that assumption: `rusqlite` is SQLite-only and synchronous, while a Postgres client is inherently network-async. Rather than define a custom `trait Store { ... }` with two hand-written implementations (duplicated query logic, duplicated bugs), `sqlx::any::AnyPool` is used: one async connection-pool type, one query surface (`sqlx::query`/`query_as` with `?` placeholders), backed by either SQLite or Postgres depending on the connection string's scheme. This is the smallest change that supports both backends correctly.

### 20.2 Backend selection

```rust
pub async fn connect(config: &Config) -> anyhow::Result<sqlx::AnyPool> {
    let url = match &config.db_url {
        Some(pg_url) => pg_url.clone(),                       // RUSTSPELL_DB_URL set
        None => format!("sqlite://{}?mode=rwc", config.db_path.display()),
    };
    let pool = sqlx::any::AnyPoolOptions::new().connect(&url).await?;
    if config.db_url.is_none() {
        sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await?;
    }
    Ok(pool)
}
```

`sqlx::any::install_default_drivers()` (or the `sqlite`+`postgres` Cargo features, which register both) must run once at startup before `connect`.

### 20.3 `Store` struct

```rust
pub struct Store {
    pool: sqlx::AnyPool,
    keys: RwLock<HashMap<String, KeyRecord>>,          // by key_hash
    tenants: RwLock<HashMap<String, TenantRecord>>,     // by tenant id
    origins_any: RwLock<HashSet<String>>,               // union of all tenants' origins — CORS predicate (§23)
    origins_by_tenant: RwLock<HashMap<String, HashSet<String>>>, // tenant id -> its origins — origin binding (§23)
}

struct TenantRecord {
    id: String,
    name: String,
    language: String,
    quota_limit: u64,
    request_count: AtomicU64,   // hot-path counter, see §24
    period_start: Option<u64>,
    period_end: Option<u64>,
    suspended_at: Option<u64>,
    created_at: u64,
}
```

`Store::open` runs schema init (§21.1 for tenants/origins, §17.1 for `api_keys` with the `tenant_id`/`platform` additions below), loads all three caches, and bootstraps a `platform` key if none exists (F22, reinterpreted: "no non-revoked `platform`-role key exists").

## 21. Tenant & Origin Store

### 21.1 Schema

Portable across SQLite and Postgres: no `AUTOINCREMENT`/`SERIAL`, `TEXT` and `BIGINT` only, `IF NOT EXISTS` throughout.

```sql
CREATE TABLE IF NOT EXISTS tenants (
    id            TEXT PRIMARY KEY,             -- uuid v4
    name          TEXT NOT NULL,
    language      TEXT NOT NULL DEFAULT 'en_US',
    quota_limit   BIGINT NOT NULL DEFAULT 0,     -- 0 == unlimited, see §21.4
    request_count BIGINT NOT NULL DEFAULT 0,
    period_start  BIGINT,
    period_end    BIGINT,
    suspended_at  BIGINT,
    created_at    BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS tenant_origins (
    id         TEXT PRIMARY KEY,                 -- uuid v4
    tenant_id  TEXT NOT NULL REFERENCES tenants(id),
    origin     TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE(tenant_id, origin)
);
CREATE INDEX IF NOT EXISTS idx_tenant_origins_origin ON tenant_origins(origin);
CREATE INDEX IF NOT EXISTS idx_tenant_origins_tenant ON tenant_origins(tenant_id);
```

`api_keys` (§17.1) gains a nullable tenant reference and the `platform` role:

```sql
ALTER TABLE api_keys ADD COLUMN tenant_id TEXT REFERENCES tenants(id); -- NULL only for role='platform'
-- role CHECK becomes: CHECK (role IN ('platform', 'admin', 'standard'))
```

(Written as a fresh `CREATE TABLE ... role TEXT NOT NULL CHECK (role IN ('platform','admin','standard')), tenant_id TEXT REFERENCES tenants(id)` in the actual migration, since this hasn't shipped yet — no live data to migrate.)

### 21.2 `Store` API additions

```rust
impl Store {
    pub async fn create_tenant(&self, req: CreateTenantRequest) -> anyhow::Result<CreatedTenant>;
    pub fn list_tenants(&self) -> Vec<TenantMetadata>;               // cache read
    pub fn get_tenant(&self, id: &str) -> Option<TenantMetadata>;    // cache read
    pub async fn update_tenant(&self, id: &str, req: UpdateTenantRequest) -> anyhow::Result<Option<TenantMetadata>>;
    pub async fn set_suspended(&self, id: &str, suspended: bool) -> anyhow::Result<bool>;

    pub async fn register_origin(&self, tenant_id: &str, origin: &str) -> anyhow::Result<OriginMetadata>;
    pub fn list_origins(&self, tenant_id: &str) -> Vec<OriginMetadata>;
    pub async fn revoke_origin(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool>; // false if id unknown/foreign

    /// Sync, in-memory only — called from the CORS predicate (§23), must not block.
    pub fn is_registered_origin(&self, origin: &HeaderValue) -> bool;
    /// Sync, in-memory only — called from `require_origin_binding` (§23).
    pub fn tenant_owns_origin(&self, tenant_id: &str, origin: &str) -> bool;
}
```

Same write-then-cache pattern as §17.3: mutations hit the pool first (`spawn`-free — `sqlx` is natively async, no `spawn_blocking` needed here, unlike the old `rusqlite` design), then update the relevant `RwLock` cache under a write guard.

### 21.3 Cache invalidation on origin changes

`register_origin`/`revoke_origin` update both `origins_any` (recompute membership: an origin might be registered by more than one tenant, e.g. a shared staging domain — only drop from `origins_any` when *no* tenant owns it anymore) and `origins_by_tenant[tenant_id]`.

### 21.4 `quota_limit = 0` means unlimited

Not "immediately blocked." Rationale: if the billing app creates a tenant without specifying a plan yet (or the field is simply omitted), the tenant should work, not silently 429 on every request — a zero-by-omission footgun is worse than an explicit opt-in cap. Billing apps that want a genuinely zero-quota (fully blocked, pre-payment) tenant should suspend it instead (F38/§22), which has clearer semantics than overloading the quota field.

## 22. Tenant HTTP Handlers & Routing

### 22.1 Router shape (`build_app`)

```rust
let platform_routes = Router::new()
    .route("/tenants", post(create_tenant).get(list_tenants))
    .route("/tenants/:id", get(get_tenant).patch(update_tenant))
    .route("/tenants/:id/suspend", post(suspend_tenant))
    .route("/tenants/:id/reactivate", post(reactivate_tenant))
    .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_platform_key));

let tenant_self_routes = Router::new()
    .route("/tenant", get(get_own_tenant))
    .route("/tenant/origins", get(list_own_origins).post(register_origin))
    .route("/tenant/origins/:id", delete(revoke_origin))
    .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_origin_binding))
    .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_active_tenant))
    .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_admin)) // GET /tenant excluded, see 22.2
    .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_active_key));
```

(`axum::middleware::Layer` order runs bottom-up per Axum's convention — `require_active_key` first, `require_admin` second, etc. — matching the ordering already established in §10.1.)

### 22.2 `GET /tenant` is the one exception to `require_admin`

Both `admin` and `standard` keys can read their own tenant's metadata (F39 explicitly says "any key from that tenant"), but only `admin` can mutate origins. `GET /tenant` is therefore routed *outside* the `tenant_self_routes` group above — it sits on `require_active_key` + `require_active_tenant` + `require_origin_binding` only, skipping `require_admin`.

## 23. Dynamic CORS & Origin Binding

Two independent checks, both origin-aware, doing different jobs:

| | Layer | Scope | Effect |
|---|---|---|---|
| CORS headers | `tower_http::cors::CorsLayer` (browser-facing) | Any origin registered by *any* tenant | Sets `Access-Control-Allow-Origin`; browser decides whether to let JS read the response |
| Origin binding | `auth::require_origin_binding` (server-facing) | The origin must belong to *this specific request's* tenant | 403 if not — a real rejection, independent of what the browser does |

### 23.1 CORS predicate

```rust
pub fn is_registered_origin(&self, origin: &HeaderValue) -> bool {
    origin
        .to_str()
        .map(|s| self.origins_any.read().unwrap().contains(s))
        .unwrap_or(false)
}
```

Synchronous, `O(1)`, no I/O — safe to call from `AllowOrigin::predicate`'s non-async closure (§10).

### 23.2 Origin binding middleware

```rust
pub async fn require_origin_binding(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let record = request.extensions().get::<KeyRecord>().expect("runs after require_active_key");
        let tenant_id = record.tenant_id.as_deref().expect("runs only on tenant-scoped routes");
        let origin_str = origin.to_str().unwrap_or_default();
        if !state.store.tenant_owns_origin(tenant_id, origin_str) {
            return AppError::Forbidden("origin not registered to this tenant".into()).into_response();
        }
    }
    next.run(request).await
}
```

No `Origin` header (server-to-server clients) → check is skipped entirely, per F43.

### 23.3 `require_platform_key` and F43a

```rust
pub async fn require_platform_key(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    // ... resolve KeyRecord as in require_active_key, but require role == Platform (403 otherwise) ...
    if request.headers().contains_key(header::ORIGIN) {
        return AppError::Forbidden("platform key not usable from a browser context".into()).into_response();
    }
    next.run(request).await
}
```

This subsumes `require_active_key` for the `/tenants*` group rather than composing with it, since the role check and the unconditional `Origin` rejection are both platform-specific (§8's routing table reflects this: `/tenants*` rows list only `require_platform_key`, not the general `require_active_key` chain).

## 24. Quota Enforcement Middleware

```rust
pub async fn require_quota(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let record = request.extensions().get::<KeyRecord>().expect("runs after require_active_key");
    let tenant_id = record.tenant_id.as_deref().expect("runs only on tenant-scoped routes");

    match state.store.try_consume_quota(tenant_id) {
        Ok(()) => next.run(request).await,
        Err(()) => AppError::QuotaExceeded.into_response(),
    }
}
```

```rust
impl Store {
    /// Atomically checks-and-increments the in-memory counter; `quota_limit == 0` always succeeds.
    /// Persists the new count fire-and-forget (same tradeoff as `touch_last_used`, §17.3 / §16 risk note).
    pub fn try_consume_quota(&self, tenant_id: &str) -> Result<(), ()> {
        let tenants = self.tenants.read().unwrap();
        let tenant = tenants.get(tenant_id).expect("tenant must exist for an authenticated key");
        if tenant.quota_limit == 0 {
            tenant.request_count.fetch_add(1, Ordering::Relaxed);
        } else {
            loop {
                let current = tenant.request_count.load(Ordering::Relaxed);
                if current >= tenant.quota_limit {
                    return Err(());
                }
                if tenant
                    .request_count
                    .compare_exchange(current, current + 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
        }
        self.flush_request_count(tenant_id); // fire-and-forget async write
        Ok(())
    }
}
```

The compare-exchange loop prevents a race where two concurrent requests both read `current == quota_limit - 1` and both increment past the limit — correctness matters more here than for `last_used_at`, since this is the actual billing enforcement boundary, not just an analytics timestamp.

## 25. Multi-Language Engine Registry (`src/engine.rs`)

```rust
pub struct EngineRegistry {
    dictionary_manager: DictionaryManager,
    engines: RwLock<HashMap<String, Arc<Engine>>>,
}

impl EngineRegistry {
    /// Eagerly loads `config.language` at startup (unchanged fail-fast behavior, F11).
    pub async fn new(config: &Config) -> anyhow::Result<Self>;

    /// Cache hit: O(1), no I/O. Cache miss: downloads + parses (§5), then caches.
    /// Concurrent misses for the same language: the second caller waits on the
    /// first's write-lock hold rather than downloading twice (double-checked locking).
    pub async fn get_or_load(&self, language: &str) -> Result<Arc<Engine>, EngineError> {
        if let Some(engine) = self.engines.read().unwrap().get(language) {
            return Ok(engine.clone());
        }
        let mut engines = self.engines.write().unwrap();
        if let Some(engine) = engines.get(language) {
            return Ok(engine.clone()); // someone else loaded it while we waited for the write lock
        }
        let (aff, dic) = self.dictionary_manager.ensure_dictionary_for(language).await?;
        let engine = Arc::new(Engine::load_from_paths(&aff, &dic)?);
        engines.insert(language.to_string(), engine.clone());
        Ok(engine)
    }
}
```

`std::sync::RwLock` held across an `.await` inside `get_or_load` is a real bug to avoid at implementation time (blocks the executor) — the write lock must be released before the `await` on `ensure_dictionary_for`, re-acquired after, with a re-check (as sketched) to handle the race where two requests both miss the cache for the same new language simultaneously. `DictionaryManager::ensure_dictionary_for(language)` is `dictionary.rs`'s existing `ensure_dictionary` (§5.1), parameterized instead of reading `config.language` directly.

`spellcheck`/`spellcheck_positions` handlers resolve the engine via `state.engines.get_or_load(req.language.as_deref().unwrap_or(&tenant.language)).await?`, mapping `EngineError` to a 400 Problem Details response (not the 500/fail-fast treatment reserved for the startup-time default language, per §5.3).

## 26. Usage Rollup & `/usage/*` Endpoints

Implements §3.10 (F47–F64). Existing observability (§11) cannot serve this: the Prometheus registry is in-process and resets on restart, and F45's `request_count` is a single running integer with no time dimension.

### 26.1 Storage

Two tables, not one. The full cross-product (day × tenant × language × status × slug × latency bucket) would multiply row count by the bucket ladder for no benefit — no endpoint needs latency split by language *and* status. Splitting keeps each query scanning only its own dimensions and keeps NF13 honest.

```sql
CREATE TABLE IF NOT EXISTS usage_daily (
    day            TEXT    NOT NULL,          -- 'YYYY-MM-DD', UTC
    tenant_id      TEXT    NOT NULL REFERENCES tenants(id),
    language       TEXT    NOT NULL,
    status         BIGINT  NOT NULL,          -- HTTP status
    error_slug     TEXT    NOT NULL,          -- AppError slug; '' for 2xx (F53: no NULLs)
    request_count  BIGINT  NOT NULL,
    latency_sum_us BIGINT  NOT NULL,          -- exact average; buckets alone only approximate
    PRIMARY KEY (day, tenant_id, language, status, error_slug)
);

CREATE TABLE IF NOT EXISTS usage_latency (
    day           TEXT    NOT NULL,
    tenant_id     TEXT    NOT NULL REFERENCES tenants(id),
    bucket_le_ms  BIGINT  NOT NULL,           -- inclusive upper bound; -1 == +Inf overflow
    request_count BIGINT  NOT NULL,
    PRIMARY KEY (day, tenant_id, bucket_le_ms)
);

CREATE INDEX IF NOT EXISTS idx_usage_daily_day ON usage_daily(day);
CREATE INDEX IF NOT EXISTS idx_usage_latency_day ON usage_latency(day);
```

Every column is `NOT NULL`, so none of them hit the `sqlx::Any` NULL-decode defect documented above `key_record_from_row` — no `COALESCE` sentinels needed here. `error_slug = ''` rather than NULL is what buys that on the one column that would naturally be nullable.

`bucket_le_ms` stores the **boundary in milliseconds, not the ladder index**. Same width, but a future ladder change leaves already-stored rows meaning exactly what they meant when written; storing an index would silently remap 90 days of history.

### 26.2 Latency bucket ladder

```rust
/// Inclusive upper bounds in ms. Dense at the low end because NF01 targets
/// p50 < 5 ms — a coarser ladder there would put p50 and p95 in the same bucket.
const LATENCY_BUCKETS_MS: [u64; 10] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000];
const BUCKET_OVERFLOW: i64 = -1; // anything above 1000 ms
```

Percentiles use the standard cumulative-count walk with linear interpolation inside the bucket that crosses the target rank. If the rank falls in the overflow bucket, report the last finite boundary (`1000`) rather than extrapolating into an unbounded range — the same thing Prometheus' `histogram_quantile` does, and the only honest answer.

Accuracy is ±half a bucket width, accepted in `REQUIREMENTS.md` §6. Buckets are `const`, not configurable: a knob that invalidates comparison against every stored row is a trap, not a feature.

### 26.3 Recording path

Recording must see only requests that passed every gate (F47), and must observe the response status, the resolved language, and the error slug. That fixes its position in the layer stack — **innermost**, wrapping the handler alone:

```
outermost / runs first
  metrics_middleware            (§11, all routes)
  request_counter
  TraceLayer / CORS / request-id
  require_active_key            ─┐
  require_active_tenant          │ added last → outermost of the group
  require_origin_binding         │
  require_quota                  │
  record_usage                  ─┘ added FIRST → innermost, wraps handler only
      spellcheck / spellcheck_positions
```

Per the `build_app` convention, `record_usage` is the **first** `.route_layer()` added to `protected_spellcheck`, making it the last to run before the handler. A request rejected by any gate above never reaches it, so rejections are never recorded — exactly F47.

The two facts the middleware cannot read off the wire travel back on response extensions:

- `resolve_engine` inserts `ResolvedLanguage(String)` after it resolves the override-or-tenant-default language.
- `AppError::into_response` inserts `ProblemSlug(&'static str)`, reusing the existing `problem_type` slug so error identifiers can never drift from the RFC 7807 `type` URIs.

A response carrying neither (a panic caught upstream, say) records `language = "unknown"`, `error_slug = "internal-error"` rather than dropping the row.

Doing this in middleware rather than at the end of each handler is deliberate: the handlers have multiple `?` early-return paths, and a call site per path would miss exactly the error cases `/usage/errors` exists to report.

### 26.4 Buffering and flush

Writing a row per request would put a serialized SQLite `UPSERT` on the hot path — the ceiling §17 already flags for `last_used_at` and `request_count`, but worse, because this write is an aggregate read-modify-write. So `UsageRecorder` accumulates in memory and a background task flushes:

```rust
pub struct UsageRecorder {
    daily:   Mutex<HashMap<DailyKey, DailyCounters>>,   // day, tenant, language, status, slug
    latency: Mutex<HashMap<LatencyKey, u64>>,           // day, tenant, bucket_le_ms
}

impl UsageRecorder {
    /// Called from `record_usage` middleware. Lock-and-increment only — no I/O,
    /// no await, so it cannot fail or slow the request (F49).
    pub fn record(&self, tenant_id: &str, language: &str, status: u16, slug: &str, latency: Duration);

    /// Drains both buffers and hands them to `Store::flush_usage`. Called every
    /// FLUSH_INTERVAL and once during graceful shutdown (§12).
    pub async fn flush(&self, store: &Store);
}

const FLUSH_INTERVAL: Duration = Duration::from_secs(10);
```

`Store::flush_usage` applies each drained entry as one `INSERT … ON CONFLICT (…) DO UPDATE SET request_count = request_count + excluded.request_count, …`, inside a single transaction. That upsert form is identical on SQLite and PostgreSQL, satisfying NF14 without a backend branch.

A crash loses at most one flush interval, which undercounts — never over-counts — exactly the tradeoff F49 permits and the same one already accepted for the quota counter.

`FLUSH_INTERVAL` is a `const`, not config. // ponytail: 10s fixed; make it configurable only if a deployment actually needs a different durability/write-volume point.

### 26.5 Authorization and scope

The existing layer groups can't express this one: F60 admits both `platform` (no tenant) and `admin` (tenant-scoped), while `require_active_tenant` assumes a tenant exists and `require_platform_key` excludes admins. One new middleware composes the existing checks rather than reimplementing them:

```rust
pub enum UsageScope { Platform, Tenant(String) }

/// Layered after `require_active_key` on the `/usage/*` group.
pub async fn require_usage_scope(...) -> Response;
```

| Caller role | Outcome |
|---|---|
| `standard` | 403 `Forbidden` (F60), consistent with F30's role-gate behaviour |
| `platform` | `Origin` header present → 403, per F43a's server-to-server rule; otherwise `UsageScope::Platform` |
| `admin` | Tenant must exist and not be suspended, and origin binding (§23.2) applies; then `UsageScope::Tenant(id)` |

Handlers read `UsageScope` from request extensions. `Platform` applies no tenant filter; `Tenant(id)` adds `WHERE tenant_id = ?` to every query. Because F57's percentage denominator is computed by the same filtered `SUM`, F61 holds structurally — there is no code path that could divide a tenant's count by a platform-wide total.

### 26.6 Window resolution

```rust
/// Returns an inclusive UTC date range.
fn resolve_window(scope: &UsageScope, start: Option<&str>, end: Option<&str>, store: &Store)
    -> Result<(NaiveDate, NaiveDate)>;
```

- Both params supplied → use them. `start > end`, an unparseable date, or a range wider than the 90-day retention window → `AppError::InvalidDateRange` (new variant, 400, slug `invalid-date-range`, with the `docs/errors/invalid-date-range.md` page the CLAUDE.md rule requires).
- Omitted, `UsageScope::Tenant` → the tenant's `period_start`/`period_end` (F59). If either is unset (the `-1` sentinel), fall back to the last 30 days rather than erroring — an un-provisioned tenant should still see its data.
- Omitted, `UsageScope::Platform` → last 30 days (F59).

Supplying only one of the two is a 400: a half-open window would silently mean different things per scope.

### 26.7 Endpoint contracts

All four are `GET`, take optional `start`/`end`, and are added as one `usage_routes` group in `build_app`. `date` is present per row when `start`/`end` were supplied and omitted otherwise (`skip_serializing_if = "Option::is_none"`), so one struct serves both F58 modes.

**`GET /usage/daily`** — always dated; a "daily" endpoint returning one undated aggregate would be nonsense. This narrows F58 for this endpoint only.

```json
{"daily_usage":[{"date":"2026-07-31","requests":100,"average_latency_ms":42,"errors":2}]}
```

`average_latency_ms` = `latency_sum_us / request_count / 1000`, exact rather than bucket-derived. `errors` counts rows with `status >= 400`.

**`GET /usage/latency`**

```json
{"latency_trends":[{"percentile":"p50","value_ms":30},{"percentile":"p95","value_ms":80},{"percentile":"p99","value_ms":150}]}
```

Interpolated per §26.2. With `start`/`end`, each row also carries `date` and percentiles are computed per day — buckets are additive, so a multi-day aggregate is a valid histogram, not an average of averages.

**`GET /usage/errors`** — both dimensions per F56.

```json
{"error_trends":[{"status":400,"error_code":"validation-error","count":12}]}
```

`error_code` is the `AppError` slug, identical to the tail of the RFC 7807 `type` URI, so a dashboard can link straight to `docs/errors/{slug}.md`.

**`GET /usage/languages`**

```json
{"language_distribution":[{"language":"en_US","count":800,"percentage":80.0}]}
```

`percentage` is rounded to one decimal and computed against the scope-filtered total (F61). An empty window returns `[]` with the percentage question moot — never a division by zero.

Requests that failed before `resolve_engine` ran (validation, malformed JSON) are attributed to the language `unknown`, since no language was ever resolved for them. That bucket is small by construction — it can only contain handler-level errors — and reporting it beats either dropping those requests from the rollup or guessing a language they never had.

All four return `200` with an empty array before any data accumulates (US22); an empty result is not a 404.

### 26.8 Retention

A second background task, on a 24-hour interval and once at startup, deletes from both tables where `day < today - 90` (F51). Two `DELETE`s in one transaction. Startup execution matters because a server that is down for a week would otherwise carry stale rows until its first interval fires.

### 26.9 `AppState` and wiring

`AppState` gains `usage: Arc<UsageRecorder>`, alongside the existing `engines`/`config`/`store`/`rate_limiter`. `main.rs` spawns the flush and purge tasks after `Store::open`, and the graceful-shutdown path (§12) awaits a final `flush` before the process exits so the last partial interval isn't lost on a clean deploy.

### 26.10 Testing

- **Bucket maths** — unit tests for percentile interpolation: known bucket counts → known p50/p95/p99, plus the overflow-bucket clamp and the empty-histogram case.
- **Persistence** — a file-backed `Store::open` → record → flush → close → reopen → query test, per the CLAUDE.md rule. `:memory:` never exercises the reload path.
- **Upsert accumulation** — flushing twice for the same key must add, not overwrite. This is the bug the `ON CONFLICT` clause exists to prevent, so it needs its own test.
- **Scope isolation** — integration test with two tenants: an admin key sees only its own counts, including in the `percentage` denominator (F61).
- **Role gate** — `standard` key → 403; `platform` key with an `Origin` header → 403.
- **Window resolution** — inverted range → 400; single param → 400; tenant with unset billing period → 30-day fallback, not an error.
- **Recording scope** — a quota-rejected (429) request records nothing, proving the layer ordering in §26.3 is correct. This is the single test most likely to catch a future refactor that moves `record_usage` outward.
- **OpenAPI** — all four paths present, spec-validation test passing (F63).

## 27. Language discovery and dictionary registration

### 27.1 `GET /languages`

Public, unauthenticated endpoint returning every language the server can serve: the union of locally cached dictionaries and registered dictionary source URLs. Each row carries `code`, `cached`, and `registered` booleans so a caller can tell whether a language is ready to use (`cached=true`), merely configured (`registered=true`), or both.

```json
{"languages":[{"code":"en_US","cached":true,"registered":false},{"code":"fr_FR","cached":false,"registered":true}]}
```

Because it is called by browsers before a tenant or API key exists, `/languages` is exempt from the per-tenant CORS origin gate (§23). The exemption is path-scoped inside the existing `AllowOrigin::predicate` rather than a second CORS layer.

### 27.2 `POST /dictionaries`

Registers a new dictionary locale and source URL template, persists it in the `dictionaries` table, then downloads, parses, and caches the `.aff`/`.dic` pair so the language is immediately available for spell-checking. Restricted to `platform` keys and, when configured, callers from `RUSTSPELL_DICTIONARY_ADMIN_CIDRS`.

The request body is validated as an RFC 7807 400 when the language code is malformed or the `source_url` is not `http(s)`. A download/parse failure during warming is also a 400 (`unsupported-language`), not a 500 — the caller supplied the URL.

### 27.3 IP allow-list and trusted proxies

`RUSTSPELL_DICTIONARY_ADMIN_CIDRS` is a comma-separated CIDR list. Empty or unset means no network restriction beyond the platform-key gate. `RUSTSPELL_TRUSTED_PROXIES` controls when the `X-Forwarded-For` header is consulted: only when the direct TCP peer falls inside a trusted proxy network. The resolved client IP walks the forwarded chain from right to left and returns the first address that is not itself a trusted proxy.

Like `/tenants*`, this endpoint rejects any request carrying an `Origin` header: it is server-to-server only.

### 27.4 Startup warming

`main.rs` pre-warms every dictionary registered in the store before binding the API port. If any registered dictionary cannot be downloaded or parsed, the process exits with a descriptive error. This keeps the fail-fast behavior already applied to the configured default language (§5.3) from being undermined by a newly registered dictionary.

### 27.5 Storage

Dictionary source URLs live in a dedicated `dictionaries` table alongside the key/tenant store, not in a sidecar file. The table is created by `Store::init_schema` and read into an in-memory cache on open, matching the pattern used for keys and tenants. The source URL is a per-code template; `DictionaryManager` substitutes `{code}.aff` and `{code}.dic` under it.

### 27.6 Testing

- Public `/languages` returns 200 without an API key.
- CORS preflight for an unregistered origin against `/languages` is allowed.
- `/languages` includes both cached-on-disk and registered-but-not-cached languages.
- `POST /dictionaries` with a `platform` key and a local HTTP source warms the language and makes it usable by `/spellcheck`.
- Non-platform keys, `Origin` headers, and out-of-CIDR IPs are rejected.
- `X-Forwarded-For` is honored only when the peer is in `RUSTSPELL_TRUSTED_PROXIES`.
- Startup warming failure path is covered by a bad source URL registered in a test store.
