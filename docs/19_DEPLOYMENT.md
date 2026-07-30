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
| `RUSTSPELL_LANGUAGE` | `en_US` | Dictionary locale |
| `RUSTSPELL_DICTIONARY_URL` | `https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en` | Base URL for `{language}.aff` and `{language}.dic` |
| `RUSTSPELL_DICTIONARY_DIR` | OS data directory | Dictionary cache path |
| `RUSTSPELL_REFRESH_INTERVAL_HOURS` | `24` | Re-download if cache is older than this |
| `RUSTSPELL_CORS_ORIGINS` | — | Comma-separated CORS allow-list (required) |

`RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` must be different.

## Running from Source

```bash
git clone https://github.com/btafoya/rustspell-server.git
cd rustspell-server
cargo build --release

RUSTSPELL_CORS_ORIGINS=http://localhost:3000 \
  ./target/release/rustspell-server
```

## Docker

```bash
docker build -t rustspell-server .
docker run -p 3000:3000 -p 9090:9090 \
  -e RUSTSPELL_CORS_ORIGINS=http://localhost:3000 \
  -v dict-cache:/data/dictionaries \
  rustspell-server
```

## Docker Compose

```bash
RUSTSPELL_CORS_ORIGINS=http://localhost:3000 docker-compose up
```

The compose file mounts a named volume at `/data/dictionaries` so dictionary downloads persist across restarts.

## Health Checks

- `GET http://localhost:3000/health` returns `{"status":"ok"}`.
- `GET http://localhost:3000/health?verbose=true` includes `uptime_seconds` and `request_count`.
- Prometheus metrics are available at `http://localhost:9090/metrics`.

## Troubleshooting

- **Address already in use**: Ensure `RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` are free and distinct.
- **Dictionary download failure**: Verify network access to `RUSTSPELL_DICTIONARY_URL` and that the base URL contains `{language}.aff` and `{language}.dic`.
- **CORS errors**: Confirm `RUSTSPELL_CORS_ORIGINS` includes the calling origin and contains no wildcard.
