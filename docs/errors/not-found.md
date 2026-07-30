# Not Found

HTTP status: `404 Not Found`

The requested `{id}` doesn't exist, or it exists but belongs to a different tenant
than the calling key. Both cases return the same `404` deliberately — returning `403`
for "exists but not yours" would let a caller enumerate other tenants' key/origin ids
by comparing status codes.

Applies to `DELETE /api-keys/{id}`, `POST /api-keys/{id}/rotate`,
`DELETE /tenant/origins/{id}`, and (platform-scoped, no cross-tenant concept there)
`GET`/`PATCH /tenants/{id}`, `POST /tenants/{id}/suspend`,
`POST /tenants/{id}/reactivate`.

Double-check the id was copied correctly and belongs to the tenant the calling key
is scoped to (`GET /api-keys` or `GET /tenant/origins` to list what actually exists).
