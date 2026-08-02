# Rust Spell Server — Requirements Specification

> Status: requirements resolved; ready for design.

## 1. Project Goal

Provide a production-ready Rust HTTP server that exposes a Hunspell-compatible spell-checking engine through a small, stable, observable REST API. The server must be deployable with minimal configuration and must satisfy the latency/throughput targets stated in the README.

## 2. Context

- `Cargo.toml` and `README.md` already describe the intended feature set.
- `src/main.rs` exists as a scaffold but references five modules (`config`, `error`, `handlers`, `models`, `nuspell`) that do not yet exist.
- `PR.md` lists the intended file layout and API surface.
- The custom CORS function in `src/main.rs` does not match Axum’s middleware contract and will not compile.
- Many declared dependencies (`metrics`, `metrics-exporter-prometheus`, `validator`, `uuid`, `tower-http` CORS/request-id features) are not yet used.

## 3. Functional Requirements

### 3.1 Endpoints

| ID | Method | Path | Description | Acceptance Criteria |
|----|--------|------|-------------|---------------------|
| F01 | GET | `/health` | Liveness/health check | Returns HTTP 200 with a JSON body indicating status; must not depend on dictionary availability unless explicitly requested. |
| F02 | GET | `/health?verbose=true` | Health check with metrics | Returns HTTP 200 with status plus basic runtime metrics. |
| F03 | GET | `/docs` | OpenAPI 3.0 specification | Returns a valid OpenAPI 3.0 JSON document describing all public endpoints. |
| F03a | GET | `/languages` | List available languages | Public endpoint returning the union of locally cached and registered dictionaries with `cached`/`registered` flags. No API key required. |
| F03b | POST | `/dictionaries` | Register a dictionary | Platform-key only (plus optional admin CIDR). Persists a language code and source URL template, then downloads/parses/caches the language so it is immediately usable. |
| F04 | POST | `/spellcheck` | Spell-check a text payload | Accepts a JSON request with `text` and/or `words`, returns one result per token occurrence. |
| F04a | POST | `/spellcheck/positions` | Spell-check with positions | Same input as `/spellcheck`, but returns unique misspelled tokens with their positions in the input. |

### 3.2 Spell-check endpoint behavior

- **F05** Request body must be validated and rejected with HTTP 400 for malformed input.
- **F06** The endpoint must accept both a `text` field and an optional `words` array; both are processed and returned as a single result set.
- **F06a** `/spellcheck` must return one result per token occurrence, preserving input order.
- **F06b** `/spellcheck/positions` must return unique misspelled tokens with the positions/indexes where each appeared.
- **F07** Response must include, at minimum: the input token, whether it is valid, and an optional list of suggestions.
- **F08** Error responses must follow RFC 7807 `application/problem+json` with `type`, `title`, `status`, and `detail` fields.

### 3.3 Spell-check engine integration

- **F09** The server must use the `spellbook` pure-Rust Hunspell-compatible engine.
- **F09a** Tokenization of `text` input must use a project-local tokenizer; `spellbook` does not expose a public tokenizer. The tokenizer strips surrounding punctuation and splits on Unicode whitespace.
- **F10** Dictionaries must be downloaded from the LibreOffice dictionaries repository at startup if not already present at a configured path; the target language must be configurable.
- **F10a** Dictionary files must be cached locally and refreshed only when the upstream version changes, checked at startup on a configurable interval (default: once per 24 hours).
- **F11** If the requested dictionary cannot be loaded or downloaded, startup must fail fast with a descriptive error.

### 3.4 Configuration

- **F12** Port: configurable via `RUSTSPELL_PORT`, default `3000`.
- **F13** Log level: configurable via `RUSTSPELL_LOG_LEVEL`, default `info`.
- **F14** Metrics port: configurable via `RUSTSPELL_METRICS_PORT`, default `9090`.
- **F15** Dictionary language: configurable via `RUSTSPELL_LANGUAGE`, default `en_US`.
- **F15a** Dictionary cache directory: configurable via `RUSTSPELL_DICTIONARY_DIR`, default to a platform-appropriate data directory.
- **F15b** Dictionary refresh interval: configurable via `RUSTSPELL_REFRESH_INTERVAL_HOURS`, default `24`.
- **F15c** Key/tenant store path: configurable via `RUSTSPELL_DB_PATH` (SQLite, default) or `RUSTSPELL_DB_URL` (PostgreSQL connection string, opt-in — see F33a).
- **F15d** Auth rate limiting: configurable via `RUSTSPELL_AUTH_RATE_LIMIT_MAX` (default `10`), `RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS` (default `60`), `RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS` (default `60`).
- **F15e** Dictionary admin CIDR allow-list: `RUSTSPELL_DICTIONARY_ADMIN_CIDRS` (comma-separated, default empty). When set, `POST /dictionaries` is additionally restricted to callers from these networks.
- **F15f** Trusted proxies: `RUSTSPELL_TRUSTED_PROXIES` (comma-separated CIDR list, default empty). Controls whether `X-Forwarded-For` is consulted when resolving the caller IP for `POST /dictionaries`.

### 3.5 Observability

- **F16** Structured JSON logging via `tracing`.
- **F17** Per-request trace logging via `tower-http::trace::TraceLayer`.
- **F18** Prometheus metrics exposed on the metrics port, including request counts, latencies, and spell-check throughput.
- **F19** Request ID propagation via `tower-http::request-id`.

### 3.6 CORS

- **F20** ~~CORS must use a configured allow-list of origins supplied at startup~~ **Superseded by §3.9**: origins are registered per tenant, not globally, once multi-tenancy lands. No wildcard fallback is permitted, at either scope.

### 3.7 Authentication & API Key Management

- **F21** `POST /spellcheck` and `POST /spellcheck/positions` require a valid API key sent via the `X-API-Key` header. `/health`, `/docs`, `/languages`, `/ui`, and `/metrics` remain unauthenticated.
- **F22** On first start, if the key store is empty, the server generates one bootstrap API key with the `platform` role (see §3.9), prints its raw value once (stdout/log), and persists only its hash. If the store is later emptied (all keys revoked/deleted), the next restart bootstraps a new platform key the same way.
- **F23** Each key has a role: `platform`, `admin`, or `standard` (see §3.9 for `platform`). `admin` keys may call both spell-check and key-management endpoints *for their own tenant*; `standard` keys may only call spell-check endpoints.
- **F24** `POST /api-keys` (admin only) creates a key given a required `label`, a `role`, and an optional `expires_at`; the raw key value is returned exactly once in the response body. Only its hash is stored.
- **F25** `GET /api-keys` (admin only) lists key metadata: `id`, `label`, `role`, `created_at`, `last_used_at`, `expires_at`, `revoked_at`. Raw and hashed values are never returned.
- **F26** `DELETE /api-keys/{id}` (admin only) soft-revokes a key by setting `revoked_at`; the row is retained for audit history. Revoked keys fail authentication immediately.
- **F27** `POST /api-keys/{id}/rotate` (admin only) issues a new raw value for an existing key row, invalidating the previous value while keeping the same `id`, `label`, and `role`.
- **F28** Keys past their `expires_at` are rejected at auth time (401) without requiring explicit revocation.
- **F29** `last_used_at` is updated on each successful authenticated request.
- **F30** A missing, invalid, expired, or revoked key on a protected endpoint returns 401 `application/problem+json`. A valid `standard` key calling an admin-only endpoint returns 403.
- **F31** Authentication failures are rate-limited per client IP via an in-memory sliding window: default 10 failures per 60s triggers 429 responses for a 60s cooldown. Thresholds are configurable via environment variables. State resets on restart (single-instance deployment).

### 3.8 Key Storage

- **F32** API keys are persisted in a database; only a salted hash of each key's raw value is stored, never the raw value itself.
- **F33** The database location/connection is configurable (see F33a), defaulting to a local SQLite file in a platform data directory, mirroring the `RUSTSPELL_DICTIONARY_DIR` pattern.
- **F33a** The persistence backend is pluggable: SQLite by default; PostgreSQL selectable via environment configuration (e.g. `RUSTSPELL_DB_URL` pointing at a `postgres://` connection string switches backends). Single server instance either way — this is about deployment flexibility, not horizontal scaling.

### 3.9 Multi-Tenancy (SaaS)

Every deployment is tenant-scoped, including a self-hosted single-org install (which is simply a platform key managing one tenant). There is no separate "single-tenant mode."

- **F34** A new `platform` role exists above `admin`/`standard`, scoped to no tenant (`tenant_id` is null). The bootstrap key from F22 is a `platform` key. Platform keys manage tenants only; they cannot call `/spellcheck*` (there is no tenant to attribute usage/quota to) or any tenant's `/api-keys*`.
- **F35** `POST /tenants` (platform only) creates a tenant and, in the same response, its first `admin`-role key (mirroring the F22 bootstrap pattern, but returned in the API response instead of printed to stdout). Required input: tenant name. Optional: default language, initial `quota_limit`, `period_start`/`period_end`.
- **F36** `GET /tenants` and `GET /tenants/{id}` (platform only) return tenant metadata: `id`, `name`, default language, `quota_limit`, `request_count`, `period_start`, `period_end`, `suspended_at`, `created_at`.
- **F37** `PATCH /tenants/{id}` (platform only) updates `quota_limit`, `period_start`/`period_end`, `name`, and/or default language.
- **F38** `POST /tenants/{id}/suspend` and `POST /tenants/{id}/reactivate` (platform only) set/clear `suspended_at`. All of a suspended tenant's keys are rejected at auth time (403) regardless of quota state.
- **F39** `GET /tenant` (any key from that tenant, i.e. `admin` or `standard`) returns the calling key's own tenant metadata (same shape as F36, minus platform-only fields if any) — self-service usage visibility without going through the billing app.
- **F40** All existing `/api-keys*` endpoints (F24–F27) are implicitly scoped to the calling `admin` key's tenant; a tenant's admin can only see/manage keys within their own tenant.
- **F41** Each tenant maintains a list of registered CORS origins (replacing the global `RUSTSPELL_CORS_ORIGINS` allow-list from F20). Origins are managed the same way as keys — via tenant-admin-authenticated endpoints (exact endpoint shape deferred to design).
- **F42** CORS `Access-Control-Allow-Origin` responses are computed dynamically per request by checking the requested origin against the union of all tenants' registered origins.
- **F43** In addition to F42's browser-side CORS headers, the server enforces origin binding: on any authenticated request that carries an `Origin` header, that origin must be one of the calling key's own tenant's registered origins, or the request is rejected (403) — this is a real server-side check, not just a response header, so a leaked key can't be replayed from an arbitrary origin via a browser. Requests without an `Origin` header (server-to-server, non-browser clients) skip this check.
- **F43a** `platform`-role keys have no tenant and therefore no registered origins to bind to. Any `/tenants*` request authenticated with a `platform` key that carries an `Origin` header is rejected (403), regardless of value — `platform` keys are for server-to-server use only (the billing app's backend, never its frontend). No CORS allow-list applies to `/tenants*` at all; it's simply unreachable from a browser.
- **F44** `POST /spellcheck` and `POST /spellcheck/positions` accept an optional `language` field per request, overriding the tenant's default language for that call. The requested language's dictionary must already be a supported/loadable language; unsupported languages return a validation error (400).
- **F45** Each tenant has a request quota: `quota_limit` (max spellcheck requests per billing period) and a `request_count` incremented on every `/spellcheck*` call attributable to that tenant (by any of its keys). Once `request_count >= quota_limit`, further spellcheck requests return 429 until the period is reset.
- **F46** The quota period (`period_start`/`period_end`) and `request_count` reset are controlled entirely by the platform/billing app via F37's `PATCH /tenants/{id}`; the server does not auto-reset the counter when `period_end` passes. A tenant whose period has lapsed without a billing-app update stays blocked.

### 3.10 Usage Metrics

The billing app's usage dashboard needs latency, error, and language history. Nothing in §3.5 provides it: Prometheus metrics are in-process and die with the process, and F45's `request_count` is a single running integer with no time dimension. This section adds a durable daily rollup and the endpoints that read it.

**Recording**

- **F47** Requests that reach a `/spellcheck*` handler are recorded into a durable daily usage rollup keyed by UTC date, tenant, resolved language, response status, and error slug. Requests rejected by the auth/tenant/origin/quota middleware are *not* recorded — the rollup counts billable work only, matching F45's `request_count` exactly.
- **F48** Each rollup row accumulates a request count plus latency histogram buckets sufficient to interpolate p50/p95/p99 for that date. Boundaries are fixed at compile time, not configurable: changing them would invalidate comparison against already-stored rows.
- **F49** Recording is non-blocking and must never fail a spellcheck request. Per the F45/F46 precedent, undercounting on crash is acceptable; over-counting is not.
- **F50** Rollup rows survive process restart and container recreation, verified by a file-backed store close/reopen test (never `:memory:`).
- **F51** Rollup rows older than 90 days are purged automatically.
- **F52** The rollup key set is bounded: date × tenant × language × status × error slug. Error slugs come from the closed `AppError` set (F08); no free-form values enter the key.
- **F53** All rollup columns are `NOT NULL` — language is always resolved because only handler-reaching requests are recorded. Any future nullable column requires the documented `sqlx::Any` `COALESCE` workaround.

**Endpoints**

- **F54** `GET /usage/daily` returns per-day request count, average latency, and error count.
- **F55** `GET /usage/latency` returns p50/p95/p99 interpolated from F48's buckets.
- **F56** `GET /usage/errors` returns counts carrying both dimensions — HTTP status *and* `AppError` slug (e.g. `{"status": 400, "error_code": "validation", "count": 12}`).
- **F57** `GET /usage/languages` returns request count and percentage-of-total per language.
- **F58** All four accept optional `start`/`end` (`YYYY-MM-DD`). Supplied → responses carry a per-date dimension for trend charts. Omitted → a single flat aggregate, except `/usage/daily`, which is always per-date (an undated "daily" aggregate is meaningless) and uses the window only to bound the range. Supplying just one of the two is a 400.
- **F59** The default window when `start`/`end` are omitted is scope-dependent: an `admin` caller gets its own tenant's current billing period (F46's `period_start`/`period_end`); a `platform` caller gets a rolling last 30 days, since billing periods differ across tenants.
- **F60** A `platform` key receives cross-tenant aggregates; an `admin` key receives its own tenant only; a `standard` key is rejected (403), consistent with F30.
- **F61** Cross-tenant data must never appear in an admin-scoped response, including in `/usage/languages` percentage denominators.
- **F62** Invalid or inverted `start`/`end` values return RFC 7807 `application/problem+json` 400 (F08) with a corresponding `docs/errors/{slug}.md` page.
- **F63** All four endpoints appear in the F03 OpenAPI spec and pass its validation test.
- **F64** F39's `GET /tenant` is unchanged: it remains the live quota-enforcement counter answering "am I near my cap?", while `/usage/*` is the historical rollup answering "what did I use, when?". The two may legitimately differ by the flush lag.

### 3.11 Language discovery and dictionary registration

- **F65** `GET /languages` is public and unauthenticated. It returns every locale that is either cached on disk or registered in the store, with `cached` and `registered` booleans.
- **F66** `/languages` is exempt from the per-tenant CORS origin gate so a browser can discover dictionaries before it has a tenant or API key.
- **F67** `POST /dictionaries` requires a `platform` key. It accepts `code` (validated language code) and `source_url` (http(s) URL template). It persists the pair, then downloads/caches/parses the language immediately.
- **F68** `RUSTSPELL_DICTIONARY_ADMIN_CIDRS` optionally restricts `POST /dictionaries` to specific source networks. `RUSTSPELL_TRUSTED_PROXIES` optionally enables `X-Forwarded-For` resolution for that IP check.
- **F69** At startup, every registered dictionary is warmed before the API port is bound. A failure to download or parse any registered dictionary fails startup fast with a descriptive error.

### 3.12 Bootstrap Platform Key Reset (CLI)

This section adds an operator escape hatch for the `platform` bootstrap key defined in F22. It is intended for the case where the bootstrap key printed on first start has been lost or compromised and the operator needs a new one without redeploying or manually editing the database.

- **F70** A CLI command named `reset-platform-key`, packaged in the same container image as the server, shall reset the active bootstrap platform key. "Reset" means rotate the key in place: same id and `label` (`"bootstrap"`), same `platform` role, new cryptographically secure raw value, and the old raw value invalidated.
- **F70a** The command shall use the same persistence backend as the server, configured via `RUSTSPELL_DB_PATH` or `RUSTSPELL_DB_URL`.
- **F71** The command must function while the server container is offline, by opening the store directly without binding the API or metrics ports, spawning background tasks, or warming dictionaries.
- **F72** If exactly one active `platform` key with `label` `"bootstrap"` exists, the command shall rotate it and print the new raw value exactly once to stdout.
- **F73** If no active `platform` key with `label` `"bootstrap"` exists, the command shall create one with label `"bootstrap"` and print the new raw value exactly once.
- **F74** If more than one active `platform` key with `label` `"bootstrap"` exists, the command shall fail with a non-zero exit code and a clear error, leaving all keys untouched.
- **F75** Rotation shall invalidate the previous raw value immediately in the store. The command must persist the new hash and update the store's key caches so the next server start sees the new key.
- **F76** The command shall require interactive confirmation unless a `--yes` flag is provided.
- **F77** On any failure the command shall return a non-zero exit code and write a descriptive message to stderr.
- **F78** The command shall not start the HTTP or metrics servers.
- **F79** Usage via `docker-compose.yml` shall require no changes to the compose file; it reuses the existing `rustspell` service image and the `data` volume (or `postgres-data` when the `postgres` profile is used). Example invocation: `docker compose run --rm rustspell reset-platform-key`.
- **F80** When `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` is set, the command shall also write the new raw key to that path in the same JSON shape as the startup bootstrap writer.
- **F81** The command shall support a `--json` flag that outputs only `{"platform_key":"..."}` (no human-readable wrapper text) and a `--quiet` flag that outputs only the raw key value. These flags are mutually exclusive with each other and with the default human-readable format.

## 4. Non-Functional Requirements

| ID | Requirement | Target |
|----|-------------|--------|
| NF01 | p50 latency for `/spellcheck` | < 5 ms |
| NF02 | p95 latency for read operations | < 50 ms |
| NF03 | Sustained throughput | > 1,000 req/s |
| NF04 | Idle RAM | < 100 MB excluding dictionary |
| NF05 | Graceful shutdown | Handle SIGINT/SIGTERM; drain in-flight requests |
| NF06 | Startup failure mode | Fail fast on missing default dictionary, invalid config, or any registered dictionary that cannot be warmed |
| NF07 | Platform support | Linux and Windows via Tokio cross-platform signal handling |
| NF08 | Test coverage | Unit tests for modules, integration tests for endpoints, benchmarks |
| NF09 | Documentation | OpenAPI spec, README, inline doc comments |
| NF10 | API key entropy | Raw key values must use a cryptographically secure RNG with sufficient length to make brute-force guessing impractical |
| NF11 | Usage recording overhead | No regression against NF01 (p50 < 5 ms), proven by the existing Criterion benchmark |
| NF12 | Usage query latency | Within NF02 (p95 < 50 ms) for a default-window query at full 90-day retention |
| NF13 | Usage storage growth | Bounded by F51/F52 — roughly 180k rows at 100 tenants × 5 languages × 4 statuses × 90 days |
| NF14 | Usage backend parity | Identical behaviour on SQLite and PostgreSQL (F33a) |

## 5. User Stories

- **US1** As an API consumer, I want to POST either a text block or a list of words so that I can know which tokens are misspelled and see suggestions.
- **US2** As an operator, I want a `/health` endpoint so that my load balancer can verify the service is alive.
- **US3** As an operator, I want Prometheus metrics on a separate port so that I can monitor the service without exposing metrics on the public API port.
- **US4** As a developer, I want an OpenAPI spec at `/docs` so that I can generate clients and understand the API contract.
- **US5** As a deployer, I want configuration via environment variables so that I can run the service in a container without modifying code.
- **US6** As a developer, I want graceful shutdown so that in-flight requests are not dropped during deploys.
- **US7** As an operator, I want a Dockerfile so that I can build and run the service in a container.
- **US8** As an operator, I want a bootstrap admin API key generated and printed on first start so that I can start managing keys without manual seeding.
- **US9** As an admin, I want to create, list, and revoke API keys so that I can control access to spell-check endpoints without redeploying.
- **US10** As an admin, I want to rotate a compromised key without losing its label or history so that I can respond to a leak quickly.
- **US11** As an operator, I want repeated auth failures throttled per client IP so that a leaked or guessed-at key can't be brute-forced.
- **US12** As a billing app (holding the platform key), I want to create a tenant and receive its first admin key in one call so that I can provision a new customer immediately after signup/payment.
- **US13** As a tenant admin, I want spellcheck requests capped by my plan's quota so that usage-based billing is enforceable, not just advisory.
- **US14** As a tenant admin, I want my own registered origins enforced server-side (not just via CORS headers) so that a leaked key can't be used from a script on an unrelated site.
- **US15** As an API consumer, I want to specify a language per spellcheck request so that one tenant can check text in multiple languages without provisioning separate tenants.
- **US16** As a billing app, I want to suspend a tenant independent of its quota so that non-payment or ToS violations can be enforced immediately.
- **US17** As a tenant admin, I want to view my own tenant's usage and plan via the API so that I don't need billing-app access just to check my quota.
- **US18** As a platform operator, I want cross-tenant latency, error, and language trends so that the billing app's usage dashboard has something to render.
- **US19** As a tenant admin, I want my own usage trends so that I can see my traffic without platform access, and without ever seeing another tenant's.
- **US20** As an operator, I want usage history to survive deploys so that the dashboard doesn't reset to zero on every release.
- **US21** As an operator, I want usage storage bounded by a retention window so that the database doesn't grow without limit.
- **US22** As a dashboard consumer, I want the usage endpoints to return empty arrays rather than errors before data accumulates, so that a fresh install renders an honest empty state instead of a 500.
- **US23** As a browser-based consumer, I want to discover available languages without an API key so that the UI can list supported dictionaries before I authenticate.
- **US24** As a platform operator, I want to register a new dictionary source URL and have the server download/cache it immediately so that tenants can use the language without a redeploy.
- **US25** As a platform operator, I want startup to fail if a registered dictionary source is broken so that runtime spell-check requests don't hit a half-loaded state.
- **US26** As an operator who has lost or leaked the bootstrap platform key, I want to run a containerized CLI command against my existing Docker Compose deployment to get a new bootstrap key, so I can regain platform access without redeploying or manually editing the database.

## 6. Decisions

| Question | Decision |
|----------|----------|
| `/spellcheck` input | Accept both `text` and `words`; process as a union. |
| `/spellcheck` result shape | One result per token occurrence, preserving order. |
| `/spellcheck/positions` result shape | Unique misspelled tokens with their positions in the input. |
| Dictionary source | Download raw `.aff`/`.dic` files from the LibreOffice dictionaries repository at startup if not cached. |
| Default language | `en_US`. |
| Tokenizer | Use project-local tokenizer (whitespace + punctuation stripping) because `spellbook` has no public tokenizer API. |
| Caching/refresh | Cache locally; check upstream for changes at startup on a configurable interval (default 24 hours). |
| Metrics | Serve Prometheus metrics on a separate TCP port (default `9090`). |
| CORS | Per-tenant registered origins, not a single global allow-list (see Multi-tenancy rows below); no wildcard fallback at either scope. |
| Authentication | API key via `X-API-Key` header, required only on `/spellcheck` and `/spellcheck/positions`. Keys are DB-backed with `platform`/`admin`/`standard` roles; a bootstrap platform key is generated and printed on first start (when the store is empty). |
| API key format | Random high-entropy token, hashed at rest (raw value shown once, never persisted or re-displayed). |
| Key management endpoints | `POST /api-keys`, `GET /api-keys`, `DELETE /api-keys/{id}` (soft revoke), `POST /api-keys/{id}/rotate` — all admin-role only, implicitly scoped to the caller's own tenant. |
| Key revocation | Soft revoke (`revoked_at` set, row retained) rather than hard delete, to preserve audit history. |
| Auth rate limiting | Per-IP in-memory sliding window on auth failures; default 10 failures/60s → 429 for 60s, configurable. |
| Key/tenant store location | SQLite file via `RUSTSPELL_DB_PATH` by default; `RUSTSPELL_DB_URL` opts into PostgreSQL. Single instance either way. |
| Multi-tenancy | Every deployment is tenant-scoped; no separate single-tenant mode. A new `platform` role (the bootstrap key) manages tenants via `/tenants*`, but cannot call `/spellcheck*` itself. |
| Tenant provisioning | Via API only (`POST /tenants`, platform key), for an external billing app — not self-service signup, not manual/out-of-band. |
| Tenant quota | Request-count-based, hard 429 block when `request_count >= quota_limit`. Period and limit are fully controlled by the platform/billing app via `PATCH /tenants/{id}`; no automatic period rollover. |
| Tenant suspension | Separate `suspended_at` flag from quota exhaustion, so non-payment/ToS actions don't have to be modeled as `quota_limit = 0`. |
| Origin binding | CORS response headers are necessary but not sufficient: the server also rejects (403) any request carrying an `Origin` header that isn't in the calling key's own tenant's registered origins. Requests without an `Origin` header are exempt. |
| Platform key + CORS | `platform` keys have no tenant/origins; `/tenants*` rejects any request carrying an `Origin` header outright (403) — server-to-server only, never called from browser JS. |
| Per-tenant language | Not fixed at tenant creation — `language` is an optional per-request override on `/spellcheck*`, since a single tenant may need multiple dictionaries. Tenant create/update still validate the default language code format. |
| Language discovery | `GET /languages` is public and unauthenticated; it unions cached files and registered source URLs with `cached`/`registered` flags. |
| Dictionary registration | `POST /dictionaries` is platform-key only with optional admin CIDR allow-list. Source URLs are persisted in the existing store (`dictionaries` table), not a sidecar file. |
| Startup warming | All registered dictionaries are warmed before binding ports; a failure fails startup fast. |
| Trusted-proxy IP resolution | `X-Forwarded-For` is honored for `POST /dictionaries` only when the direct peer is in `RUSTSPELL_TRUSTED_PROXIES`; the chain is walked from right to left to find the first non-trusted client IP. |

| Platform support | Linux and Windows via cross-platform Tokio signal handling. |
| Error format | RFC 7807 `application/problem+json`. |
| OpenAPI spec | Hand-written static spec plus a validation test. |
| Spell-check engine | `spellbook` (pure Rust, Hunspell-compatible). |
| Deployment | Include a Dockerfile and docker-compose.yml. |
| Usage data source | A durable daily rollup table in the store, not the Prometheus registry (in-process, wiped on restart) and not an external TSDB (extra infra dependency). All four `/usage/*` endpoints derive from the one table. |
| Usage recording scope | `/spellcheck*` handler-reaching requests only, so rollup counts equal billable requests. Consequence: `/usage/errors` never shows 401/403/429 — those are rejected upstream by middleware — so it covers `validation`, malformed-JSON, and `internal` only. Accepted; Prometheus and logs cover rejection debugging. |
| Latency percentiles | Fixed logarithmic-ish ms bucket ladder per row, percentiles interpolated at query time. Bounded storage and mergeable across dates and tenants, at ±half a bucket width. Reservoir sampling rejected (samples don't merge across tenants); sum/count/max rejected (percentiles not derivable). |
| Usage retention | 90 days, purged automatically — enough for month-over-month comparison at single-digit MB. |
| Usage default window | Admin scope: the caller's current billing period, so it lines up with the quota counter. Platform scope: rolling last 30 days, since "current billing period" is undefined across many tenants. |
| Usage error identifiers | Both HTTP status and `AppError` slug per row — `400` alone doesn't distinguish validation from malformed JSON. |
| Usage backfill | None. Charts fill in from first deploy; seeding a synthetic row from `request_count` would date a lifetime total to one day and carry no latency, language, or status breakdown. |

## 7. Remaining Open Questions

No major product questions remain, including for API key authentication (§3.7–3.8), SaaS multi-tenancy (§3.9), and usage metrics (§3.10). Two deferred design details:
- The exact shape of the per-tenant CORS-origin management endpoints (F41) — CRUD-equivalent to `/api-keys`, but not spec'd field-by-field here; resolve in `/sc:design`.
- The bootstrap key reset CLI in §3.12: exact command name, output modes, and whether to mirror the capability as a platform-only HTTP endpoint; resolve in `/sc:design`.

The next step is architecture/design.

**External dependency (out of scope here):** the `spellcheckapi.com` PHP app must inject a platform- or admin-authenticated HTTP client into `RustMetricsService` and add the four routes to its `e2e/mock-api-server.js`. The endpoints are inert until it does. Tracked in that repository, not this one.

## 7. Known Blockers

- `src/config.rs`, `src/error.rs`, `src/handlers.rs`, `src/models.rs`, and the `src/spellbook/` (or equivalent) module tree do not exist.
- The CORS middleware in `src/main.rs` is not valid Axum middleware and will not compile.
- `Cargo.toml` declares `nuspell-sys = "0.1"`, which is not a usable published crate; it must be replaced with `spellbook`.
- No tests, integration harness, or benchmark file exists yet.
- The metrics endpoint is not implemented.

## 8. Recommended Next Step

Resolve the open questions above, then proceed to `/sc:design` to produce an architecture and implementation plan.
