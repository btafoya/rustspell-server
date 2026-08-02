# Invalid Date Range

HTTP status: `400 Bad Request`

The `start`/`end` window supplied to a `/usage/*` endpoint could not be used. The
`detail` field names which rule was broken:

- **Malformed date** — both values must be exactly `YYYY-MM-DD`. Impossible dates
  (`2026-02-30`, `2025-02-29`) are rejected, not silently rolled forward.
- **`start` after `end`** — the range is inclusive and must be ordered.
- **Only one of the two supplied** — a half-open window would mean different
  things depending on the calling key's scope, so both are required together.
- **Range wider than the retention window** — rollup rows are purged after 90
  days, so a longer span could only ever return partial data.

Omit both parameters to get the default window instead: an `admin` key gets its
own tenant's current billing period, and a `platform` key gets the last 30 days.

```bash
# Explicit window
curl -H "X-API-Key: $KEY" \
  "https://api.example.com/usage/daily?start=2026-07-01&end=2026-07-31"

# Default window
curl -H "X-API-Key: $KEY" https://api.example.com/usage/daily
```

An empty result is not an error: before any usage accumulates, these endpoints
return `200` with an empty array.
