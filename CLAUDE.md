# Rust Spell Server — Project Instructions

These instructions override the global `CLAUDE.md` for this repository only.

## Project Context

- Rust HTTP server exposing a Hunspell-compatible spell-checking API.
- Built with `axum`, `tokio`, `spellbook` (pure-Rust Hunspell-compatible engine, version `0.4.2`).
- Serves two TCP ports: public API (`RUSTSPELL_PORT`, default `3000`) and Prometheus metrics (`RUSTSPELL_METRICS_PORT`, default `9090`).
- Dictionaries are downloaded as raw `.aff`/`.dic` files from a URL template (`RUSTSPELL_DICTIONARY_URL`/`{language}.aff`) and cached locally — not `.oxt` archives.

## Architecture Decisions (Locked)

| Area | Decision | Source |
|------|----------|--------|
| Spell-check engine | `spellbook = "0.4.2"` published crate | `DESIGN.md` §2.1, §6 |
| Dictionary source | Raw `.aff`/`.dic` downloads from a URL template, not `.oxt` archives | `REQUIREMENTS.md` |
| Tokenization | Project-local tokenizer (whitespace + punctuation stripping); `spellbook` has no public tokenizer | `DESIGN.md` §6.1 |
| CORS | Per-tenant registered origins, dynamic `AllowOrigin::predicate` via `tower_http::cors::CorsLayer`; no global allow-list | `DESIGN.md` §10, §23 |
| Errors | RFC 7807 `application/problem+json` | `DESIGN.md` §9 |
| OpenAPI | Hand-written static spec + validation test | `REQUIREMENTS.md` |
| Auth | API key via `X-API-Key`, required only on `/spellcheck*`; DB-backed store (SQLite default, PostgreSQL via `RUSTSPELL_DB_URL`), multi-tenant with `platform`/`admin`/`standard` roles, bootstrap platform key printed on first start | `REQUIREMENTS.md` §3.7–3.9, `DESIGN.md` §17–25 |
| Shutdown | Cross-platform `ctrl_c` + Unix `SIGTERM` | `DESIGN.md` §12 |
| Metrics | Separate TCP port (`9090`) | `DESIGN.md` §11 |

## When Implementing

- Follow the module layout in `DESIGN.md` §2.
- Follow the implementation order in `DESIGN.md` §15.
- Do not reintroduce `nuspell-sys`; it was removed because no usable published crate exists.
- Do not use a wildcard CORS fallback. `RUSTSPELL_CORS_ORIGINS` no longer exists — origins are per-tenant, managed via `/tenant/origins*` (`DESIGN.md` §21–23).
- Keep handlers thin; business logic belongs in `src/engine.rs` and `src/dictionary.rs`.
- Maintain `AppState` as `Arc<EngineRegistry>` + `Arc<Config>` + `Arc<Store>` + `Arc<RateLimiter>` shared read-only state.
- Tower/axum layer stacking: the *last* `.layer()`/`.route_layer()` added is outermost and runs *first* on a request — opposite of "added first runs first." Order middleware accordingly (see `handlers::build_app` for the pattern).
- New `AppError` variant → add a matching `docs/errors/{slug}.md` page (the RFC 7807 `type` field links there).

## Testing Requirements

- Unit tests for config, validation, tokenizer, and error mapping.
- Integration tests using Axum/Tower test utilities (no real port binding).
- Benchmark in `benches/spellcheck_bench.rs` using Criterion.
- OpenAPI validation test that `/docs` matches the spec snapshot.
- `sqlx::Any` + SQLite: `AnyRow::try_get::<Option<T>, _>()` unreliably decodes NULL values read back from a real (file-backed) database — reproduces only on a *second* `Store::open`, never with `:memory:`. Workaround already applied in `src/store.rs`: `COALESCE` nullable columns to a sentinel in the `SELECT`, decode as non-null. Any new nullable column needs the same treatment.
- Any change touching `Store`/SQLite persistence needs a test that closes and reopens a **file-backed** store, not `:memory:` (see `store::tests::reopen_file_backed_store_preserves_data`) — in-memory tests never exercise the reload-from-disk path.
- For deployment/persistence changes, verify with a real `docker compose up` → create data → `--force-recreate` → confirm data survived. `cargo test` alone missed a bug where every tenant/key was lost on container restart.

## Commit and Attribution

- Use human authorship only; no AI attribution in commits or code.
- Commit author: `Brian Tafoya <btafoya@briantafoya.com>`.
- Keep messages terse and descriptive.
- Do not commit secrets, `.env` files, or downloaded dictionary files.

## Sources of Truth

- Requirements: `REQUIREMENTS.md`
- Architecture: `DESIGN.md`
- Entry point: `src/main.rs`
- Dependencies: `Cargo.toml`

If a requirement conflicts with this file, update `REQUIREMENTS.md` or `DESIGN.md` first before changing code.
