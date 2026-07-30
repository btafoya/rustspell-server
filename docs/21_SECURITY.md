# Security

## Threat Model

Rust Spell Server is a stateless spell-checking API. The primary security concerns are:

- Ensuring only configured origins can access the API via CORS.
- Returning safe, structured error responses that do not leak internal details.
- Preventing resource exhaustion through input validation and reasonable request size limits.
- Keeping the dependency tree auditable and free of unnecessary native TLS/OpenSSL where possible.

## Implemented Controls

### CORS

- CORS is enforced via `tower_http::cors::CorsLayer`.
- Only origins listed in `RUSTSPELL_CORS_ORIGINS` are allowed.
- No wildcard fallback is permitted.

### Error Handling

- All errors are returned as RFC 7807 `application/problem+json` responses with `type`, `title`, `status`, and `detail`.
- Internal error details are logged server-side but not returned to clients.

### Input Validation

- `text` is limited to 10,000 characters.
- `words` arrays are limited to 1,000 entries.
- At least one of `text` or `words` must be provided.

### Observability

- Structured JSON logging via `tracing`.
- Per-request trace logging via `tower-http::trace::TraceLayer`.
- Request ID propagation via `tower-http::request-id`.

### Dependencies

- The spell-check engine is pure Rust (`spellbook`), avoiding FFI/native library risks.
- HTTP client and Prometheus exporter are configured to use `rustls-tls`, removing the runtime OpenSSL dependency.
- Run `cargo audit` regularly to check for known vulnerabilities.

## Authentication

Authentication is intentionally not implemented for this initial release. If you deploy in an untrusted environment, place the service behind an authenticating reverse proxy or API gateway.

## Reporting Issues

If you discover a security issue, please open a private GitHub issue or email the maintainer directly.
