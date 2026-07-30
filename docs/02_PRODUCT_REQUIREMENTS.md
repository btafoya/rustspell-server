# Product Requirements

## Roles

- **API Consumer**: Sends text or word lists to be spell-checked and receives per-token or positional results.
- **Operator**: Deploys and monitors the service, configures ports, CORS origins, and dictionary sources.
- **Contributor**: Maintains or extends the codebase, tests, and documentation.

## Scale

- Single-process Tokio server with a read-only shared dictionary.
- Two TCP listeners: public API (default 3000) and Prometheus metrics (default 9090).
- Designed for container deployment with a mounted dictionary cache volume.

## Navigation

| Document | Purpose |
|----------|---------|
| [Vision](01_VISION.md) | Why this project exists |
| [Product Requirements](02_PRODUCT_REQUIREMENTS.md) | This document |
| [Technical Overview](27_TECHNICAL_OVERVIEW.md) | Architecture, build instructions, developer docs |
| [Deployment](19_DEPLOYMENT.md) | How to run your own instance |
| [Security](21_SECURITY.md) | Security model and practices |
| [Contribution Guide](25_CONTRIBUTING.md) | How to contribute |
| [Coding Standards](22_CODING_STANDARDS.md) | Code style and review expectations |

## Functional Requirements Summary

- `GET /health` and `GET /health?verbose=true` for liveness and runtime metrics.
- `GET /docs` returns the OpenAPI 3.0 JSON specification.
- `POST /spellcheck` accepts `text` and/or `words` and returns one result per token occurrence.
- `POST /spellcheck/positions` returns unique misspelled tokens with their character positions in the input text.
- Errors are returned as RFC 7807 `application/problem+json`.
- Dictionaries are downloaded from a configurable base URL, cached locally, and refreshed based on age.

## Non-Functional Requirements Summary

- p50 < 5 ms for `/spellcheck`
- p95 < 50 ms for read operations
- > 1,000 req/s sustained
- < 100 MB RAM at idle (excluding dictionary)
- Graceful shutdown on SIGINT/SIGTERM
- Fail-fast startup on invalid config or missing dictionary
