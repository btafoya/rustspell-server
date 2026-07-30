# Quota Exceeded

HTTP status: `429 Too Many Requests`

The calling tenant has reached its `quota_limit` of `/spellcheck*` requests for the
current billing period. This is distinct from [Too Many Requests](rate-limited.md),
which is about repeated auth *failures*, not successful usage.

There's no `Retry-After` header — waiting doesn't help. The server never
auto-resets the counter when a period ends (by design: the billing/platform app
controls this explicitly). Resolution is one of:

- The tenant's `platform`-role owner raises `quota_limit` via `PATCH /tenants/{id}`.
- The `platform`-role owner resets `request_count` (e.g. `0`) via the same endpoint
  on a billing period rollover.

Check current usage with `GET /tenant` (any key on the tenant) or
`GET /tenants/{id}` (platform key).
