# Deployment

## Requirements

- Rust 1.70+ (for building from source)
- Docker 20.10+ (for container deployment)
- Docker Compose (for compose deployment)

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUSTSPELL_PORT` | `3000` | Public API port |
| `RUSTSPELL_METRICS_PORT` | `9090` | Prometheus metrics port |
| `RUSTSPELL_LOG_LEVEL` | `info` | `tracing` log filter |
| `RUSTSPELL_LANGUAGE` | `en_US` | Default dictionary locale (per-tenant, overridable per request) |
| `RUSTSPELL_DICTIONARY_URL` | `https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en` | Base URL for `{language}.aff` and `{language}.dic` |
| `RUSTSPELL_DICTIONARY_DIR` | OS data directory | Dictionary cache path |
| `RUSTSPELL_REFRESH_INTERVAL_HOURS` | `24` | Re-download if cache is older than this |
| `RUSTSPELL_DB_PATH` | OS data directory | SQLite file for the key/tenant store, used when `RUSTSPELL_DB_URL` is unset |
| `RUSTSPELL_DB_URL` | — | PostgreSQL connection string (`postgres://...`); takes precedence over `RUSTSPELL_DB_PATH` when set |
| `RUSTSPELL_AUTH_RATE_LIMIT_MAX` | `10` | Auth failures allowed per IP per window before a cooldown |
| `RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS` | `60` | Sliding window (seconds) for counting auth failures |
| `RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS` | `60` | Cooldown (seconds) once the failure threshold is exceeded |

`RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` must be different. There is no
`RUSTSPELL_CORS_ORIGINS` — CORS is per-tenant, registered via the API
(`POST /tenant/origins`), not a startup env var. See [Authentication &
Tenants](#authentication--tenants) below.

## Running from Source

```bash
git clone https://github.com/btafoya/rustspell-server.git
cd rustspell-server
cargo build --release

./target/release/rustspell-server
```

No environment variables are required to start. On first run, watch stdout for the
bootstrap platform API key — see [Authentication & Tenants](#authentication--tenants).

## Authentication & Tenants

Every deployment is tenant-scoped, including a single self-hosted instance — there is
no auth-free mode. On first start (whenever the key store has no active `platform`
key), the server prints a bootstrap platform key once:

```
Bootstrap platform API key (save this now, it will not be shown again):
  rsk_08e8683182474a99a58039e9d48b0bb642b0fada012e49caa4bee5ca97411a77
```

This key is not persisted anywhere retrievable and is never shown again — losing it
before provisioning at least one tenant means restarting the server against an empty
key store to get a new one. Use it to create your first tenant:

```bash
PLATFORM_KEY=rsk_...

curl -X POST http://localhost:3000/tenants \
  -H "content-type: application/json" \
  -H "x-api-key: $PLATFORM_KEY" \
  -d '{"name": "My App"}'
```

The response includes the tenant and its first `admin` key. Use that admin key to
mint further keys (`POST /api-keys`), register the origins your browser clients call
from (`POST /tenant/origins`), and check usage (`GET /tenant`). Full endpoint list in
[README.md](README.md#api-endpoints).

The `platform` key manages tenants only (`/tenants*`) and is rejected outright if a
request to `/tenants*` carries an `Origin` header — it's for server-to-server use
(e.g. a billing backend), never a browser. Keep it out of any client-side code.

## Storage

The key/tenant store defaults to a local SQLite file (`RUSTSPELL_DB_PATH`). Set
`RUSTSPELL_DB_URL` to a `postgres://` connection string to use PostgreSQL instead —
either way this remains a single-instance deployment; the choice is about operational
preference (e.g. an existing Postgres fleet), not horizontal scaling.

**The store must live on persistent storage.** Every tenant and API key lives in this
file/database — losing it means every issued key stops working and every tenant
config (quota, registered origins) is gone. See [Docker](#docker) below for the
volume-mounting gotcha this implies.

## Docker

```bash
docker build -t rustspell-server .
docker run -p 3000:3000 -p 9090:9090 \
  -v rustspell-data:/data \
  rustspell-server
```

Mount the **whole** `/data` directory, not just `/data/dictionaries`. The image sets
both `RUSTSPELL_DICTIONARY_DIR=/data/dictionaries` and
`RUSTSPELL_DB_PATH=/data/rustspell.db` — mounting only the dictionaries subdirectory
leaves the SQLite store on the container's writable layer, silently losing every
tenant and API key on the next `docker rm`/recreate.

Watch `docker logs` on first start for the bootstrap platform key.

## Docker Compose

```bash
docker compose up
```

The compose file mounts a single named volume at `/data`, covering both the
dictionary cache and the SQLite store.

To run the key/tenant store on PostgreSQL instead, start the optional `postgres`
profile and point `RUSTSPELL_DB_URL` at it:

```bash
docker compose --profile postgres up
```

## Reverse Proxy

Terminate TLS and forward to the public API port (`3000`) from a reverse
proxy; don't expose the metrics port (`9090`) publicly — scrape it from
inside your network, or proxy it separately behind auth if external access is
required. Register the proxy's public origin via `POST /tenant/origins` for
any tenant whose browser clients call the API directly — there is no CORS
env var to update anymore.

### Caddy

```
spellcheck.example.com {
    reverse_proxy localhost:3000
}
```

Caddy handles TLS certificate acquisition and renewal automatically.

### Nginx

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

# Optional: internal-only access to metrics
server {
    listen 9090;
    server_name spellcheck.example.com;
    allow 10.0.0.0/8;
    deny all;

    location /metrics {
        proxy_pass http://localhost:9090;
    }
}
```

Note: the per-IP auth-failure rate limiter (`RUSTSPELL_AUTH_RATE_LIMIT_*`) sees
whatever IP the connection arrives from. Behind this kind of proxy, that's the
proxy's IP for every client unless you additionally configure trusted-proxy
`X-Forwarded-For` handling — out of scope for this server today, so all proxied
clients currently share one rate-limit bucket.

## Health Checks

- `GET http://localhost:3000/health` returns `{"status":"ok"}`.
- `GET http://localhost:3000/health?verbose=true` includes `uptime_seconds` and `request_count`.
- Prometheus metrics are available at `http://localhost:9090/metrics`.

## Troubleshooting

- **Address already in use**: Ensure `RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` are free and distinct.
- **Dictionary download failure**: Verify network access to `RUSTSPELL_DICTIONARY_URL` and that the base URL contains `{language}.aff` and `{language}.dic`.
- **Lost the bootstrap platform key / no tenants exist yet**: Restart the server against
  the same (still-empty) key store — it bootstraps a new platform key on any start
  where no active `platform` key exists.
- **Tenants/keys disappeared after a restart**: Almost always a volume-mounting
  problem — confirm the whole `/data` directory (not just `/data/dictionaries`) is on
  persistent storage. See [Storage](#storage).
- **401 Unauthorized**: Missing/invalid/expired/revoked `X-API-Key`. See
  [`docs/errors/unauthorized.md`](docs/errors/unauthorized.md).
- **403 Forbidden on a spellcheck/tenant call from a browser**: The calling origin
  isn't registered to that key's tenant (`POST /tenant/origins`), or the tenant is
  suspended. See [`docs/errors/forbidden.md`](docs/errors/forbidden.md).
- **429 on `/spellcheck*`**: Either the auth-failure rate limiter or the tenant's
  quota — the response `type` URL distinguishes them. See
  [`docs/errors/rate-limited.md`](docs/errors/rate-limited.md) and
  [`docs/errors/quota-exceeded.md`](docs/errors/quota-exceeded.md).
