# Vision

## Why Rust Spell Server Exists

Rust Spell Server provides a fast, reliable, and easy-to-operate HTTP spell-checking service backed by a pure-Rust Hunspell-compatible engine. The project exists to give API consumers a low-latency, production-ready spell-check endpoint without requiring a native C/C++ dependency chain or manual dictionary management.

## Goals

- **Performance**: p50 latency under 5 ms and sustained throughput above 1,000 req/s for single-word checks.
- **Simplicity**: Run as a single binary with configuration via environment variables.
- **Observability**: Expose Prometheus metrics and structured logs out of the box.
- **Standards Compliance**: Ship a hand-written OpenAPI 3.0 spec and RFC 7807 error responses.
- **Operational Robustness**: Download and cache dictionaries automatically, fail fast on configuration or dictionary errors, and shut down gracefully on SIGINT/SIGTERM.

## Non-Goals

- Advanced natural-language processing or grammar checking.
- User management, authentication, or multi-tenancy for this initial release.
- Wildcard CORS fallback; operators must explicitly configure allowed origins.
