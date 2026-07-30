# Forbidden

HTTP status: `403 Forbidden`

The key authenticated successfully but isn't allowed to perform this action. Common
causes:

- A `standard` key called an `admin`-only endpoint (`/api-keys*`, `/tenant/origins*`).
- A `platform` key was used against `/tenants*` with an `Origin` header present —
  `platform` keys are for server-to-server use only and are rejected outright if a
  request looks like it came from a browser, regardless of the origin's validity.
- The request carried an `Origin` header that isn't registered to the calling key's
  own tenant (`POST /tenant/origins`), even if that origin is registered to some
  *other* tenant.
- The calling tenant is suspended (`POST /tenants/{id}/suspend`).

This is distinct from [Not Found](not-found.md): a `404` on `{id}` routes means the id
doesn't exist or belongs to a different tenant; a `403` here means the id (or route)
itself isn't accessible to this key at all.
