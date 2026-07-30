# Unsupported Language

HTTP status: `400 Bad Request`

The `language` field on `POST /spellcheck` or `POST /spellcheck/positions` was
well-formed (matches `^[A-Za-z0-9_-]{1,20}$`) but couldn't actually be loaded —
either the download from `RUSTSPELL_DICTIONARY_URL` failed (wrong locale code, no
`{language}.aff`/`{language}.dic` at that path) or the downloaded files failed to
parse as a valid Hunspell dictionary.

This is a `400`, not a `500`: a bad `language` value is a client input problem, not a
server fault, and doesn't get the fail-fast/crash treatment the server's own
startup-time default language does.

Omit `language` entirely to use the tenant's default (`GET /tenant` shows it), or
double-check the locale code against the
[LibreOffice dictionaries repository](https://github.com/LibreOffice/dictionaries)
(or your configured `RUSTSPELL_DICTIONARY_URL`, if overridden).
