# Coding Standards

## General Principles

- **Incremental progress**: small changes that compile and pass tests.
- **Learn from existing code**: study three similar features/components before adding new ones.
- **Pragmatic over dogmatic**: adapt to project reality.
- **Clear intent over clever code**: be boring and obvious.

## Rust-Specific Conventions

- Use `cargo fmt` for formatting.
- Use `cargo clippy` and resolve all warnings.
- Prefer `thiserror` for error enums and `anyhow` for application-level propagation.
- Use `Arc` for shared read-only state.
- Avoid `unsafe` unless absolutely necessary.

## Error Handling

- Fail fast with descriptive messages at the appropriate level.
- Never silently swallow exceptions.
- Map application errors to RFC 7807 `application/problem+json` responses in `src/error.rs`.

## Testing

- Test behavior, not implementation.
- Use scenario-based test names.
- Integration tests must use in-memory Axum/Tower utilities; do not bind real ports.

## Documentation

- Add inline doc comments for public APIs.
- Keep `README.md` and `*.md` files accurate.
- Update `openapi.json` when the API contract changes.

## Performance

- The dictionary is immutable after load and shared behind `Arc`.
- Avoid unnecessary allocations in hot paths.
- Run `cargo bench` before and after performance-sensitive changes.
