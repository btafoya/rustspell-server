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

### 3.5 Observability

- **F16** Structured JSON logging via `tracing`.
- **F17** Per-request trace logging via `tower-http::trace::TraceLayer`.
- **F18** Prometheus metrics exposed on the metrics port, including request counts, latencies, and spell-check throughput.
- **F19** Request ID propagation via `tower-http::request-id`.

### 3.6 CORS

- **F20** CORS must use a configured allow-list of origins supplied at startup; no wildcard fallback is permitted.

## 4. Non-Functional Requirements

| ID | Requirement | Target |
|----|-------------|--------|
| NF01 | p50 latency for `/spellcheck` | < 5 ms |
| NF02 | p95 latency for read operations | < 50 ms |
| NF03 | Sustained throughput | > 1,000 req/s |
| NF04 | Idle RAM | < 100 MB excluding dictionary |
| NF05 | Graceful shutdown | Handle SIGINT/SIGTERM; drain in-flight requests |
| NF06 | Startup failure mode | Fail fast on missing dictionary or invalid config |
| NF07 | Platform support | Linux and Windows via Tokio cross-platform signal handling |
| NF08 | Test coverage | Unit tests for modules, integration tests for endpoints, benchmarks |
| NF09 | Documentation | OpenAPI spec, README, inline doc comments |

## 5. User Stories

- **US1** As an API consumer, I want to POST either a text block or a list of words so that I can know which tokens are misspelled and see suggestions.
- **US2** As an operator, I want a `/health` endpoint so that my load balancer can verify the service is alive.
- **US3** As an operator, I want Prometheus metrics on a separate port so that I can monitor the service without exposing metrics on the public API port.
- **US4** As a developer, I want an OpenAPI spec at `/docs` so that I can generate clients and understand the API contract.
- **US5** As a deployer, I want configuration via environment variables so that I can run the service in a container without modifying code.
- **US6** As a developer, I want graceful shutdown so that in-flight requests are not dropped during deploys.
- **US7** As an operator, I want a Dockerfile so that I can build and run the service in a container.

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
| CORS | Always require an explicit configured allow-list of origins. |
| Authentication | None for this PR. |
| Platform support | Linux and Windows via cross-platform Tokio signal handling. |
| Error format | RFC 7807 `application/problem+json`. |
| OpenAPI spec | Hand-written static spec plus a validation test. |
| Spell-check engine | `spellbook` (pure Rust, Hunspell-compatible). |
| Deployment | Include a Dockerfile and docker-compose.yml. |

## 7. Remaining Open Questions

No major product questions remain. The next step is architecture/design.

## 7. Known Blockers

- `src/config.rs`, `src/error.rs`, `src/handlers.rs`, `src/models.rs`, and the `src/spellbook/` (or equivalent) module tree do not exist.
- The CORS middleware in `src/main.rs` is not valid Axum middleware and will not compile.
- `Cargo.toml` declares `nuspell-sys = "0.1"`, which is not a usable published crate; it must be replaced with `spellbook`.
- No tests, integration harness, or benchmark file exists yet.
- The metrics endpoint is not implemented.

## 8. Recommended Next Step

Resolve the open questions above, then proceed to `/sc:design` to produce an architecture and implementation plan.
