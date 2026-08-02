# Rust Spell Server

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-orange)](https://www.rust-lang.org)
[![spellbook](https://img.shields.io/badge/spellbook-0.4.2-blue)](https://crates.io/crates/spellbook)

A production-ready Rust HTTP server exposing a spell-checking API over OpenAPI-compliant endpoints.

## Motivation

I build several applications that each need spell-checking, and I was tired of
bolting on a different ad-hoc solution for every one of them. Rust Spell
Server exists to be that shared piece: a single, fast, self-hosted spell-check
API any of my apps can call over HTTP instead of vendoring a spell-check
library per project.

## Features

- **Fast**: Sub-50 ms p95 latency for spell-check operations
- **Type-Safe**: Strongly-typed request/response models with validation
- **Multi-Tenant**: API-key auth with `platform`/`admin`/`standard` roles, per-tenant
  request quotas, and per-tenant CORS origin registration
- **Observability**: Structured logging, Prometheus metrics, distributed tracing
- **Production-Ready**: Graceful shutdown, signal handling, pluggable SQLite/PostgreSQL storage
- **OpenAPI Compliant**: Interactive Swagger UI at `/ui` and raw spec at `/docs`

## Quick Start

```bash
# Clone and build
git clone https://github.com/btafoya/rustspell-server.git
cd rustspell-server
cargo build --release

# Run the server — no configuration required to start
./target/release/rustspell-server
```

On first start (whenever the key store has no active `platform` key), the server
prints a **bootstrap platform API key** once, to stdout:

```
Bootstrap platform API key (save this now, it will not be shown again):
  rsk_08e8683182474a99a58039e9d48b0bb642b0fada012e49caa4bee5ca97411a77
```

Save it — it's not persisted anywhere retrievable and won't be shown again. Use it to
provision your first tenant:

```bash
PLATFORM_KEY=rsk_...   # the key printed above

curl -X POST http://localhost:3000/tenants \
  -H "content-type: application/json" \
  -H "x-api-key: $PLATFORM_KEY" \
  -d '{"name": "My App"}'
```

The response includes the new tenant and its first `admin` key. Use that admin key to
mint further `admin`/`standard` keys (`POST /api-keys`) and register browser origins for
CORS (`POST /tenant/origins`), or use it directly against `/spellcheck`:

```bash
ADMIN_KEY=rsk_...   # from the tenant creation response above

curl -X POST http://localhost:3000/spellcheck \
  -H "content-type: application/json" \
  -H "x-api-key: $ADMIN_KEY" \
  -d '{"words": ["hello", "wrld"]}'
```

The `platform` key manages tenants only — it cannot call `/spellcheck*` (there's no
tenant to attribute usage to) and is rejected outright if a request to `/tenants*`
carries an `Origin` header, since it's meant for server-to-server use only (e.g. a
billing backend), never a browser.

## API Endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/` | none | Redirects to `/ui` |
| GET | `/ui` | none | Interactive Swagger UI documentation portal |
| GET | `/health` | none | Liveness/health check |
| GET | `/health?verbose=true` | none | Health check with uptime and request count |
| GET | `/docs` | none | OpenAPI 3.0 specification (JSON) |
| POST | `/spellcheck` | admin/standard | Spell-check text and/or word list |
| POST | `/spellcheck/positions` | admin/standard | Misspelled tokens with char positions |
| POST | `/api-keys` | admin | Create a key for your own tenant |
| GET | `/api-keys` | admin | List your own tenant's keys |
| DELETE | `/api-keys/{id}` | admin | Revoke a key |
| POST | `/api-keys/{id}/rotate` | admin | Issue a new value for an existing key |
| GET | `/tenant` | admin/standard | Read your own tenant's metadata/usage |
| GET | `/tenant/origins` | admin | List your own tenant's registered CORS origins |
| POST | `/tenant/origins` | admin | Register a CORS origin |
| DELETE | `/tenant/origins/{id}` | admin | Revoke a registered origin |
| POST | `/tenants` | platform | Create a tenant + its first admin key |
| GET | `/tenants` | platform | List all tenants |
| GET | `/tenants/{id}` | platform | Get one tenant |
| PATCH | `/tenants/{id}` | platform | Update quota, billing period, name, language |
| POST | `/tenants/{id}/suspend` | platform | Suspend a tenant (blocks all its keys) |
| POST | `/tenants/{id}/reactivate` | platform | Reactivate a suspended tenant |
| GET | `/usage/daily` | platform/admin | Per-day requests, average latency, errors |
| GET | `/usage/latency` | platform/admin | p50/p95/p99 latency |
| GET | `/usage/errors` | platform/admin | Counts by HTTP status and error slug |
| GET | `/usage/languages` | platform/admin | Requests and share of total per language |

### Usage metrics

The four `/usage/*` endpoints read a rollup written on every billable
`/spellcheck*` request and retained for 90 days. A `platform` key sees every
tenant; an `admin` key sees only its own.

All four accept an optional `start`/`end` window (`YYYY-MM-DD`, both or
neither). With a window, rows carry a `date` so the data plots as a trend;
without one, they return a single aggregate over the caller's current billing
period (admin) or the last 30 days (platform). `/usage/daily` is always dated.

```bash
curl -H "X-API-Key: $KEY" localhost:3000/usage/latency
curl -H "X-API-Key: $KEY" "localhost:3000/usage/daily?start=2026-07-01&end=2026-07-31"
```

Two things to know when reading the numbers:

- Only requests that reach a spellcheck handler are counted, so the totals
  match billable usage exactly — but `/usage/errors` never shows `401`, `403`,
  or `429`, since those are rejected by middleware first. It covers validation,
  malformed-JSON, and internal errors only. Use the Prometheus endpoint on
  `:9090` for rejection rates.
- Requests that fail before the language is resolved are attributed to the
  language `unknown` in `/usage/languages`.

Percentiles are interpolated from fixed histogram buckets, so they are accurate
to about half a bucket width rather than exact.

All authenticated endpoints take the key via the `X-API-Key` header. Open a browser to
`/` after starting the server for interactive documentation, or fetch the raw spec from
`/docs`.

## Configuration

Environment variables:

See `.env.example` for a copy-pasteable template of every variable below.

| Variable | Default | Description |
|----------|---------|-------------|
| `RUSTSPELL_PORT` | `3000` | Public API port |
| `RUSTSPELL_METRICS_PORT` | `9090` | Prometheus metrics port |
| `RUSTSPELL_LOG_LEVEL` | `info` | `tracing` log filter |
| `RUSTSPELL_LANGUAGE` | `en_US` | Dictionary locale |
| `RUSTSPELL_DICTIONARY_URL` | `https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en` | Base URL for `{language}.aff` and `{language}.dic` |
| `RUSTSPELL_DICTIONARY_DIR` | OS data directory | Dictionary cache path |
| `RUSTSPELL_REFRESH_INTERVAL_HOURS` | `24` | Re-download if cache is older than this |
| `RUSTSPELL_DB_PATH` | OS data directory | SQLite file for the key/tenant store, used when `RUSTSPELL_DB_URL` is unset |
| `RUSTSPELL_DB_URL` | — | PostgreSQL connection string (`postgres://...`); takes precedence over `RUSTSPELL_DB_PATH` when set |
| `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` | — | Path to write `{"platform_key":"..."}` when a bootstrap platform key is created |
| `RUSTSPELL_AUTH_RATE_LIMIT_MAX` | `10` | Auth failures allowed per IP per window before a cooldown |
| `RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS` | `60` | Sliding window (seconds) for counting auth failures |
| `RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS` | `60` | Cooldown (seconds) once the failure threshold is exceeded |

`RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` must be different. There's no
`RUSTSPELL_CORS_ORIGINS` — CORS is per-tenant, managed via `POST /tenant/origins`
(see [Quick Start](#quick-start)), not a startup env var.

### Bootstrap Secrets File

For automated provisioning and testing, the server can write the freshly created
bootstrap platform key to a JSON file:

```bash
RUSTSPELL_BOOTSTRAP_SECRETS_PATH=/run/secrets/bootstrap.json ./target/release/rustspell-server
```

The file is only written when a new platform key is bootstrapped, and contains:

```json
{ "platform_key": "rsk_..." }
```

### Resetting the Bootstrap Key

If the bootstrap platform key is lost or compromised, run the offline
`reset-platform-key` subcommand against the same store. Stop the running
server container first, then:

**Docker Compose**

```bash
docker compose run --rm rustspell reset-platform-key --yes
```

**Plain Docker**

```bash
docker run --rm -v rustspell-data:/data rustspell-server reset-platform-key --yes
```

This rotates the existing `bootstrap` platform key in place (same id/label,
new raw value) and prints the new key once. Add `--json` for machine-readable
output (`{"platform_key":"..."}`) or `--quiet` for just the raw value. If
`RUSTSPELL_BOOTSTRAP_SECRETS_PATH` is set, the new key is also written there.

The command does not start the HTTP server or warm dictionaries; it opens the
store directly and exits.

## Live API Testing

A live-call test suite validates every operation in `openapi.json` against a
real server process over HTTP. The suite is gated by the `live-tests` Cargo
feature and is written entirely in Rust.

```bash
cargo test --features live-tests --test live_api_tester
```

By default the harness builds/spawns the server on random free ports, creates a
fresh SQLite database, and tears everything down after the scenarios complete.
Alternatively, point it at an already-running server:

```bash
RUSTSPELL_SERVER_URL=http://localhost:3000 \
RUSTSPELL_PLATFORM_KEY=rsk_... \
cargo test --features live-tests --test live_api_tester
```

The suite can also produce JSON and JUnit reports:

```bash
RUSTSPELL_TEST_REPORT=all \
RUSTSPELL_TEST_REPORT_DIR=target/live-test-reports \
cargo test --features live-tests --test live_api_tester
```

### Docker Testing

Run the live suite inside a single container:

```bash
docker build -f Dockerfile.test -t rustspell-tester .
docker run --rm rustspell-tester
```

Or run it against a separate server container:

```bash
docker compose -f docker-compose.test.yml up --build --exit-code-from rustspell-tester
```

## Metrics

Prometheus metrics are served on a separate port (`RUSTSPELL_METRICS_PORT`,
default `9090`) at `/metrics` — not on the public API port.

```bash
curl http://localhost:9090/metrics
```

Exposed metrics:

| Metric | Type | Labels | Description |
|--------|------|--------|--------------|
| `http_requests_total` | counter | `method`, `path`, `status` | Total requests processed |
| `http_request_duration_seconds` | histogram | `method`, `path` | Request latency |
| `spellcheck_tokens_total` | counter | — | Tokens checked across all `/spellcheck*` calls |
| `dictionary_refresh_total` | counter | `result` | Dictionary download/refresh attempts |

### Prometheus scrape config

```yaml
scrape_configs:
  - job_name: rustspell-server
    static_configs:
      - targets: ["localhost:9090"]
```

## Deployment

### Docker

```bash
docker build -t rustspell-server .
docker run -p 3000:3000 -p 9090:9090 \
  -v rustspell-data:/data \
  rustspell-server
```

Mount the whole `/data` directory, not just `/data/dictionaries` — the SQLite
key/tenant store lives at `/data/rustspell.db` (both paths are set via
`RUSTSPELL_DICTIONARY_DIR`/`RUSTSPELL_DB_PATH` in the image). Mounting only the
dictionaries subdirectory would silently lose every tenant and API key on each
container restart.

Watch `docker logs` on first start for the bootstrap platform key (see
[Quick Start](#quick-start)).

### Docker Compose

```bash
docker compose up
```

To run the key/tenant store on PostgreSQL instead of the bundled SQLite, start the
optional `postgres` profile and point `RUSTSPELL_DB_URL` at it:

```bash
docker compose --profile postgres up
```

### Reverse Proxy

Put a reverse proxy in front of the public API port (`3000`) for TLS
termination. Keep the metrics port (`9090`) off the public internet — scrape
it from inside your network instead. Register the proxy's public origin via
`POST /tenant/origins` for any tenant whose browser clients call the API
directly — CORS is per-tenant now, not a startup env var.

**Caddy** (`Caddyfile`):

```
spellcheck.example.com {
    reverse_proxy localhost:3000
}
```

**Nginx**:

```nginx
server {
    listen 443 ssl;
    server_name spellcheck.example.com;

    ssl_certificate     /etc/letsencrypt/live/spellcheck.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/spellcheck.example.com/privkey.pem;

    location / {
        proxy_pass http://localhost:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

See [Deployment](DEPLOYMENT.md#reverse-proxy) for details.

## Documentation

- [Product Requirements](REQUIREMENTS.md) — roles, scale, navigation
- [Deployment](DEPLOYMENT.md) — how to run your own instance
- [Security](SECURITY.md) — security model and practices
- [Contribution Guide](CONTRIBUTING.md) — how to contribute

## Contributing

Contributions are welcome. Please read the
[Contribution Guide](CONTRIBUTING.md) and
[Coding Standards](CODING_STANDARDS.md) before opening issues or pull
requests.

## Security

Rust Spell Server uses API key authentication (`X-API-Key`, hashed at rest) with
per-tenant isolation, per-tenant CORS origin registration enforced both via browser
CORS headers and a server-side origin-binding check, per-IP rate limiting on auth
failures, RFC 7807 structured error responses, structured logging with request-id
propagation, and dependency auditing. See [Security](SECURITY.md) for details.

## Performance Targets

- p50 < 5 ms for `/spellcheck`
- p95 < 50 ms for read operations
- > 1,000 req/s sustained
- < 100 MB RAM at idle (excluding dictionary)

## License

This project is licensed under the [MIT License](LICENSE).
