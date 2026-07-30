# Rust Spell Server — Project Instructions

These instructions override the global `CLAUDE.md` for this repository only.

## Project Context

- Rust HTTP server exposing a Hunspell-compatible spell-checking API.
- Built with `axum`, `tokio`, `spellbook` (pure-Rust Hunspell-compatible engine, version `0.4.2`).
- Serves two TCP ports: public API (`RUSTSPELL_PORT`, default `3000`) and Prometheus metrics (`RUSTSPELL_METRICS_PORT`, default `9090`).
- Dictionaries are downloaded from LibreOffice extension `.oxt` archives and cached locally.

## Architecture Decisions (Locked)

| Area | Decision | Source |
|------|----------|--------|
| Spell-check engine | `spellbook = "0.4.2"` published crate | `DESIGN.md` §2.1, §6 |
| Dictionary source | LibreOffice extension `.oxt` downloads | `REQUIREMENTS.md` |
| Tokenization | Project-local tokenizer (whitespace + punctuation stripping); `spellbook` has no public tokenizer | `DESIGN.md` §6.1 |
| CORS | Configured allow-list only, via `tower_http::cors::CorsLayer` | `DESIGN.md` §10 |
| Errors | RFC 7807 `application/problem+json` | `DESIGN.md` §9 |
| OpenAPI | Hand-written static spec + validation test | `REQUIREMENTS.md` |
| Auth | None for this PR | `REQUIREMENTS.md` |
| Shutdown | Cross-platform `ctrl_c` + Unix `SIGTERM` | `DESIGN.md` §12 |
| Metrics | Separate TCP port (`9090`) | `DESIGN.md` §11 |

## When Implementing

- Follow the module layout in `DESIGN.md` §2.
- Follow the implementation order in `DESIGN.md` §15.
- Do not reintroduce `nuspell-sys`; it was removed because no usable published crate exists.
- Do not use a wildcard CORS fallback; `RUSTSPELL_CORS_ORIGINS` is required.
- Keep handlers thin; business logic belongs in `src/engine.rs` and `src/dictionary.rs`.
- Maintain `AppState` as `Arc<Engine>` + `Arc<Config>` shared read-only state.

## Testing Requirements

- Unit tests for config, validation, tokenizer, and error mapping.
- Integration tests using Axum/Tower test utilities (no real port binding).
- Benchmark in `benches/spellcheck_bench.rs` using Criterion.
- OpenAPI validation test that `/docs` matches the spec snapshot.

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
