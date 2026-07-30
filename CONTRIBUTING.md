# Contribution Guide

## Getting Started

1. Fork the repository: https://github.com/btafoya/rustspell-server
2. Clone your fork and create a feature branch.
3. Make your changes with tests.
4. Run the full test and lint suite.
5. Submit a pull request.

## Development Workflow

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets
cargo bench --no-run
```

All checks must pass before a PR is merged.

## Code Style

- Follow standard Rust formatting (`cargo fmt`).
- Keep functions small and focused.
- Prefer composition over inheritance and explicit data flow over clever code.
- Match the comment density and naming of the surrounding code.

## Testing

- Add unit tests for new logic in the relevant module.
- Add integration tests in `tests/integration.rs` for new endpoints or behaviors.
- Add benchmarks in `benches/spellcheck_bench.rs` for performance-sensitive changes.

## Commit Messages

Keep messages terse and descriptive. Describe what changed and why. Example:

```
Add rate limit middleware to public API
Update dictionary manager to support custom refresh intervals
```

## Documentation

Update `README.md` and relevant `*.md` files when behavior, configuration, or deployment instructions change.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
