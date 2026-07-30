# Dictionary Parse Error

HTTP status: `503 Service Unavailable`

The dictionary's `.aff`/`.dic` files were downloaded (or read from cache) but failed
to parse as a valid Hunspell-format dictionary. For the server's startup-time default
language this is a fail-fast condition; for a per-request `language` override on
`/spellcheck*` it maps to [Unsupported Language](unsupported-language.md) (`400`)
instead.

This usually means the configured `RUSTSPELL_DICTIONARY_URL` served something that
isn't a genuine Hunspell `.aff`/`.dic` pair (wrong path, redirected to an HTML error
page, corrupted cache) — verify the URL directly, or clear `RUSTSPELL_DICTIONARY_DIR`
to force a fresh download.
