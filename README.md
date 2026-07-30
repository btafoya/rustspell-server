# Rust Spell Server

A production-ready Rust HTTP server exposing a Hunspell-compatible spell-checking API over OpenAPI-compliant endpoints.

## Features

- **Fast**: Sub-50 ms p95 latency for spell-check operations
- **Type-Safe**: Strongly-typed request/response models with validation
- **Observability**: Structured logging, Prometheus metrics, distributed tracing
- **Production-Ready**: Graceful shutdown, signal handling, CORS allow-list
- **OpenAPI Compliant**: API documentation served at `/docs`

## Quick Start

```bash
# Clone and build
git clone https://github.com/your-org/rustspell-server.git
cd rustspell-server
cargo build --release

# Run the server
RUSTSPELL_CORS_ORIGINS=http://localhost:3000 ./target/release/rustspell-server
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness/health check |
| GET | `/health?verbose=true` | Health check with uptime and request count |
| GET | `/docs` | OpenAPI 3.0 specification |
| POST | `/spellcheck` | Spell-check text and/or word list |
| POST | `/spellcheck/positions` | Misspelled tokens with char positions |

See the full API specification at `/docs` after starting the server.

## Configuration

Environment variables:

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

## Deployment

### Docker

```bash
docker build -t rustspell-server .
docker run -p 3000:3000 -p 9090:9090 \
  -e RUSTSPELL_CORS_ORIGINS=http://localhost:3000 \
  -v dict-cache:/data/dictionaries \
  rustspell-server
```

### Docker Compose

```bash
RUSTSPELL_CORS_ORIGINS=http://localhost:3000 docker-compose up
```

## Performance Targets

- p50 < 5 ms for `/spellcheck`
- p95 < 50 ms for read operations
- > 1,000 req/s sustained
- < 100 MB RAM at idle (excluding dictionary)

## License

MIT License - see LICENSE file for details.
