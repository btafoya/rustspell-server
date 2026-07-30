# Unauthorized

HTTP status: `401 Unauthorized`

The `X-API-Key` header was missing, the key doesn't exist, or it has expired or been
revoked.

- Confirm the `X-API-Key` header is present and set to a raw key value returned by
  `POST /api-keys` or `POST /tenants` (never the key `id`).
- If the key was revoked (`DELETE /api-keys/{id}`) or rotated
  (`POST /api-keys/{id}/rotate`), the old raw value stops working immediately —
  use the new one.
- If the key had an `expires_at`, check whether it has passed.

Repeated failed attempts from the same IP are rate-limited — see
[Too Many Requests](rate-limited.md).
