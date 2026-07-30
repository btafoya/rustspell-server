# Live API Tester Requirements

## Goal
Create a self-contained, live-call API validation suite for the Rust Spell Server. It starts a real server process and exercises every operation declared in the hand-written `openapi.json` over actual TCP HTTP calls. The suite runs under `cargo test --features live-tests` and is also runnable inside Docker.

## Scope
Validate the implementation against the hand-written OpenAPI 3.0.3 spec (`openapi.json`). For every `operationId`:
- Exercise the success path and assert HTTP status + response body matches the declared schema and explicit happy-path values.
- Exercise every declared non-success status code and assert the response is a valid RFC 7807 `application/problem+json` body.

The suite must first parse `openapi.json`, list all `operationId`s, and fail if any operationId lacks a corresponding live test.

## Architecture

### Test runner
- Shell-based using `bats` (Bash Automated Testing System) and `curl`.
- A Rust integration test file (`tests/live_api_tester.rs`) acts as the harness when the `live-tests` feature is enabled.
- The harness:
  - Builds/runs the server binary (`cargo run --bin rustspell-server` by default, or a path from `RUSTSPELL_SERVER_BIN`).
  - Allocates a random free TCP port for the public API and a second random free port for metrics.
  - Creates a fresh file-backed SQLite database in a temp directory and deletes it after the run.
  - Waits for `GET /health` to return 200.
  - Runs the `bats` suite.
  - Tears down the server.
- If `bats` or `curl` are missing when the feature is enabled, the test fails with a clear error.

### Server bootstrap key capture
- The server writes a JSON file to the path given by `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` only when the database is freshly created.
- Format: `{"platform_key": "<raw platform key>"}`.
- The harness reads this file and exports `RUSTSPELL_PLATFORM_KEY` for the bats tests.
- If the file is missing or empty, the test fails with a clear message.

### Dictionary handling
- Tests use a real public dictionary download URL for realism.
- The harness sets `RUSTSPELL_DICTIONARY_URL` to that URL.
- The server retries failed downloads with exponential backoff up to a configurable timeout (default 60s).
- Downloaded dictionaries are cached across test runs in a temp directory to avoid repeat network traffic.
- If the dictionary remains unloadable, the suite fails with a clear message.

### Test isolation
- One server process serves the entire suite.
- Tests run serially to avoid state collisions.
- The SQLite database starts empty for each run; no reset between individual bats files.
- Each stateful bats file uses `@setup` to create the tenant/key state it needs and `@teardown` to clean it up.

### Test organization
- Stateless contract tests: one bats file per tag/topic (e.g., `health.bats`, `docs.bats`, `spellcheck.bats`) with tests for success and error status codes.
- Stateful lifecycle tests: dedicated bats files with setup/teardown (e.g., `api_keys_lifecycle.bats`, `tenant_lifecycle.bats`, `origins_lifecycle.bats`).
- A manifest file or discovery rule lists every `operationId` and maps it to the bats test(s) covering it. The Rust harness verifies complete coverage before running.

### External deployment mode
- If `RUSTSPELL_SERVER_URL` is set, the harness skips spawning a server and runs bats against that URL.
- In external mode the caller must also provide `RUSTSPELL_PLATFORM_KEY`.
- Docker compose mode does not support external targets; it always starts both the server and tester services.

## Reporting
- Default output: bats TAP/console output to stdout/stderr.
- `RUSTSPELL_TEST_REPORT` controls additional report formats: `console` (default), `json`, `junit`, `all`.
- `RUSTSPELL_TEST_REPORT_DIR` sets the output directory (default `target/live-test-reports/`).
- JSON report contains: run timestamp, server URL, each operationId tested, requested/actual status codes, pass/fail, and any schema deviations.
- JUnit report contains test cases grouped by bats file.

## Error validation
- Every non-2xx response must be checked for `Content-Type: application/problem+json`.
- Problem details must include `type`, `title`, `status`, and `detail`.
- `type` must match the documented `type` URI for the specific error where the spec/doc provides one.

## Docker support
- `Dockerfile.test`: builds the server and test runner in one image; entrypoint is `run_tests.sh`.
- `docker-compose.test.yml`: builds a `rustspell-server` service and a `rustspell-tester` service; the tester waits for the server to be healthy, then runs the bats suite against `http://rustspell-server:3000`.
- Both artifacts must use the same temp-dir/database cleanup semantics and report directory handling.

## Gating
- The entire live tester is behind a Cargo feature: `live-tests`.
- Without the feature, `cargo test` skips the live tests entirely and they do not require `bats` or `curl`.
- With the feature enabled, missing `bats` or `curl` causes a hard test failure, not a skip.

## Assumptions
- The server binary can write a bootstrap secrets JSON file when `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` is set and the database is new.
- The server supports binding to `0.0.0.0:<port>` via `RUSTSPELL_PORT` and `RUSTSPELL_METRICS_PORT` env vars.
- A public dictionary URL is reachable from the test environment.
- `bats` and `curl` are available in CI/Docker images.

## Open questions
1. Which public dictionary URL should be the default? (e.g., a GitHub raw URL for `en_US` `.aff`/`.dic` files.)
2. Should the server retry logic already exist, or is it part of this work?
3. Should the manifest/coverage check be a separate `tests/live_api_coverage.rs` test or built into the harness?
4. Are there existing bats helpers/conventions in the repo to match?
