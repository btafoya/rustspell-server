# Security

## Threat Model

Rust Spell Server is a multi-tenant spell-checking API backed by a persistent
key/tenant store. The primary security concerns are:

- Ensuring a key can only ever act within its own tenant's data (keys, origins, usage) —
  including that failed lookups don't leak whether another tenant's resource exists.
- Ensuring only browser origins a tenant has explicitly registered can read responses,
  and that a leaked key can't be replayed from an arbitrary origin via a browser.
- Keeping the `platform` role (tenant provisioning, no tenant scope of its own)
  unusable from any browser context, since it's the highest-privilege role and has no
  per-tenant blast-radius limit.
- Storing API keys such that a database leak doesn't hand out working credentials.
- Preventing brute-force key guessing and resource exhaustion (auth-failure rate
  limiting, per-tenant request quotas, input size limits).
- Returning safe, structured error responses that do not leak internal details.
- Keeping the dependency tree auditable and free of unnecessary native TLS/OpenSSL
  where possible.

## Implemented Controls

### Authentication

- Every request to `/spellcheck*`, `/api-keys*`, `/tenant*`, and `/tenants*` requires a
  valid API key via the `X-API-Key` header. `/health`, `/docs`, `/ui`, and `/metrics`
  are unauthenticated.
- Keys are random, high-entropy tokens (`rsk_` + 2×UUIDv4, ~244 bits). Only a SHA-256
  hash of the raw value is ever stored — the raw value is returned exactly once, at
  creation or rotation, and is not retrievable again. A fast hash (not a slow KDF like
  bcrypt/argon2) is deliberate: these are server-generated random tokens, not
  low-entropy user passwords, so the threat is leakage, not offline brute-force
  guessing.
- Three roles: `platform` (manages tenants, no tenant of its own), `admin` and
  `standard` (both tenant-scoped; `admin` additionally manages that tenant's keys and
  registered origins). A `standard` key can never escalate to `admin` or reach another
  tenant's data.
- Keys can carry an `expires_at`; expired keys are rejected the same as revoked ones.
  Revocation is soft-delete (`revoked_at` set, row retained) for audit history.

### Authorization & Tenant Isolation

- Every `{id}`-addressed resource (`/api-keys/{id}`, `/tenant/origins/{id}`) is scoped
  to the calling key's own tenant. An id that exists but belongs to a different
  tenant returns `404`, not `403` — a `403` would confirm the id exists elsewhere,
  letting a caller enumerate other tenants' key/origin ids by comparing status codes.
- A suspended tenant (`POST /tenants/{id}/suspend`) has all of its keys rejected
  (`403`) on every tenant-scoped route, independent of quota state.
- The `platform` role cannot call `/spellcheck*` (there's no tenant to attribute usage
  to) or any tenant's `/api-keys*`/`/tenant/origins*`.

### CORS & Origin Binding

CORS response headers and server-side origin binding are two independent checks —
the first is a browser-side convenience, the second is the actual security boundary:

- `Access-Control-Allow-Origin` is computed dynamically per request against the union
  of all tenants' registered origins (`POST /tenant/origins`). There is no global
  allow-list and no wildcard fallback.
- Separately, any authenticated request carrying an `Origin` header is checked
  server-side: that origin must be registered to *the calling key's own tenant*, or
  the request is rejected (`403`) — even if the origin is validly registered to some
  *other* tenant. This is real enforcement, not just a response header: setting the
  CORS header alone would only stop a compliant browser from letting page JS read the
  response; it would not stop the request from being processed, or a non-browser
  client from replaying a leaked key with an arbitrary `Origin` value.
- `platform`-role keys are exempt from origin binding in a stricter way: any request
  to `/tenants*` carrying an `Origin` header at all is rejected (`403`) outright,
  regardless of the key's validity or the origin's registration status. `platform`
  keys are server-to-server only (e.g. a billing backend) and should never appear in
  browser-reachable code; this makes that unconditional, not just documented policy.
- Requests without an `Origin` header (server-to-server clients) skip origin binding
  entirely — it only applies to browser-context requests.

### Rate Limiting & Quota

- Per-IP sliding-window rate limiting on authentication *failures* (missing, invalid,
  expired, or revoked key): default 10 failures per 60 seconds triggers a 60-second
  cooldown (`RUSTSPELL_AUTH_RATE_LIMIT_*`). Successful requests never count against
  this.
- Per-tenant request quotas on `/spellcheck*` (`quota_limit`, `0` = unlimited),
  enforced with a compare-and-increment inside a single lock critical section so
  concurrent requests near the limit cannot overshoot it. Distinct `429` cause from
  rate limiting (different `type` URL, no `Retry-After` — quota resolution is a
  billing action via `PATCH /tenants/{id}`, not something to wait out).

### Input Validation

- `text` is limited to 10,000 characters; `words` arrays to 1,000 entries; at least
  one of `text` or `words` must be provided.
- The per-request `language` override is restricted to `^[A-Za-z0-9_-]{1,20}$`. This
  is deliberately stricter than real-world locale-code syntax: `language` is
  interpolated into both a filesystem cache path and a dictionary-download URL, so an
  unvalidated value (e.g. `../../etc/passwd`) would be a path-traversal vector into
  the dictionary cache directory. Validated before it ever reaches the dictionary
  loader.
- A syntactically valid but unloadable `language` (bad locale code, download/parse
  failure) returns `400`, not `500` — a client input problem is not a server fault,
  and doesn't get the fail-fast/crash treatment the server's own startup-time default
  language does.

### Error Handling

- All errors are returned as RFC 7807 `application/problem+json` responses with
  `type`, `title`, `status`, and `detail`. The `type` field disambiguates causes that
  share an HTTP status (e.g. rate-limited vs. quota-exceeded, both `429`). See
  [`docs/errors/`](docs/errors/) for the full list.
- Internal error details are logged server-side but not returned to clients.

### Observability

- Structured JSON logging via `tracing`.
- Per-request trace logging via `tower-http::trace::TraceLayer`.
- Request ID propagation via `tower-http::request-id`.
- Prometheus metrics on a separate port from the public API, including
  `auth_attempts_total` broken out by result (success/invalid/expired/revoked/rate
  limited).

### Dependencies

- The spell-check engine is pure Rust (`spellbook`), avoiding FFI/native library risks.
- HTTP client and Prometheus exporter are configured to use `rustls-tls`, removing the
  runtime OpenSSL dependency.
- The SQLite backend is a bundled/vendored build (no system `libsqlite3` dependency to
  track separately); PostgreSQL is opt-in via `RUSTSPELL_DB_URL`.
- Run `cargo audit` regularly to check for known vulnerabilities.

## Known Limitations

- **Reverse-proxy IP visibility**: the per-IP auth-failure rate limiter sees whatever
  IP the connection arrives from. Behind a reverse proxy, that's the proxy's IP for
  every client unless trusted-proxy `X-Forwarded-For` handling is added — not
  implemented today, so all proxied clients currently share one rate-limit bucket.
- **Admin lockout is intentional**: if every `admin` key for a tenant (or every
  `platform` key) is revoked while the server is running, there is no way to mint a
  replacement until restart — bootstrap only runs at startup. This is a deliberate
  choice (no unauthenticated runtime escape hatch), called out here so it isn't
  mistaken for a bug.
- **Single-instance deployment**: both the SQLite and PostgreSQL backends assume one
  running server instance. The in-memory auth-failure rate limiter and per-tenant
  quota counter are not shared across instances.

## Reporting Issues

If you discover a security issue, please open a private GitHub issue or email the maintainer directly.
