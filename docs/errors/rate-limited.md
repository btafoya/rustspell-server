# Too Many Requests (Rate Limited)

HTTP status: `429 Too Many Requests`

Too many failed authentication attempts (missing, invalid, expired, or revoked key)
from this client IP in a short window. This is distinct from
[Quota Exceeded](quota-exceeded.md), which is about a tenant's successful-request
budget, not auth failures.

The response includes a `Retry-After` header (seconds) — wait at least that long
before retrying. Defaults: 10 failures per 60 seconds triggers a 60-second cooldown
(configurable via `RUSTSPELL_AUTH_RATE_LIMIT_MAX`,
`RUSTSPELL_AUTH_RATE_LIMIT_WINDOW_SECONDS`, `RUSTSPELL_AUTH_RATE_LIMIT_COOLDOWN_SECONDS`).

If you're hitting this unexpectedly, double-check the `X-API-Key` value being sent —
this usually means a client is retrying with a bad key rather than backing off.
