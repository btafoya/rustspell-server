# Dictionary Download Error

HTTP status: `503 Service Unavailable`

The server couldn't download a dictionary's `.aff`/`.dic` files from
`RUSTSPELL_DICTIONARY_URL`. For the server's startup-time default language, this is a
fail-fast condition — the server won't start (see server logs). For a per-request
`language` override on `/spellcheck*`, this maps to
[Unsupported Language](unsupported-language.md) (`400`) instead of a `503`, since it's
scoped to one request rather than the whole service.

Common causes: network connectivity to the configured dictionary host, an incorrect
`RUSTSPELL_DICTIONARY_URL`, or the upstream repository being temporarily unavailable.
Retries with exponential backoff already happen internally for transient
connect/timeout errors before this is surfaced.
