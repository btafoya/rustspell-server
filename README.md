# Rust Spell Server

[![Build Status](https://img.shields.io/github/actions/workflow/status/btafoya/rustspell-server/ci.yml?branch=main)](https://github.com/btafoya/rustspell-server/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-orange)](https://www.rust-lang.org)
[![spellbook](https://img.shields.io/badge/spellbook-0.4.2-blue)](https://crates.io/crates/spellbook)

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
git clone https://github.com/btafoya/rustspell-server.git
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

## Documentation

- [Vision](docs/01_VISION.md) — why this project exists
- [Product Requirements](docs/02_PRODUCT_REQUIREMENTS.md) — roles, scale, navigation
- [Technical Overview](docs/27_TECHNICAL_OVERVIEW.md) — architecture, build
  instructions, and developer docs
- [Deployment](docs/19_DEPLOYMENT.md) — how to run your own instance
- [Security](docs/21_SECURITY.md) — security model and practices
- [Contribution Guide](docs/25_CONTRIBUTING.md) — how to contribute

## Contributing

Contributions are welcome. Please read the
[Contribution Guide](docs/25_CONTRIBUTING.md) and
[Coding Standards](docs/22_CODING_STANDARDS.md) before opening issues or pull
requests.

## Security

Rust Spell Server uses CORS allow-list validation, RFC 7807 structured error
responses, structured logging with request-id propagation, rate-limit friendly
Prometheus metrics, and dependency auditing. See [Security](docs/21_SECURITY.md)
for details.

## Performance Targets

- p50 < 5 ms for `/spellcheck`
- p95 < 50 ms for read operations
- > 1,000 req/s sustained
- < 100 MB RAM at idle (excluding dictionary)

## License

This project is licensed under the [MIT License](LICENSE).
