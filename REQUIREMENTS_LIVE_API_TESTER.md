# Live API Tester Requirements

## Goal
Create a self-contained, live-call API validation suite for the Rust Spell
Server. It starts a real server process and exercises every operation declared
in the hand-written `openapi.json` over actual TCP HTTP calls. The suite runs
under `cargo test --features live-tests` and is also runnable inside Docker.

## Scope
Validate the implementation against the hand-written OpenAPI 3.0.3 spec
(`openapi.json`). For every `operationId`:

- Exercise the success path and assert HTTP status + response body matches the
  declared schema and explicit happy-path values.
- Exercise every declared non-success status code and assert the response is a
  valid RFC 7807 `application/problem+json` body.

The suite parses `openapi.json`, lists all `operationId`s, and fails if any
operationId lacks a corresponding live test.

## Architecture

### Test runner

- Pure Rust, with no shell scripting or external test runners.
- A single integration test file (`tests/live_api_tester.rs`) acts as the
  harness when the `live-tests` feature is enabled.
- The harness:
  - Builds or locates the server binary (`cargo build --bin rustspell-server`
    by default, or a path from `RUSTSPELL_SERVER_BIN`).
  - Allocates a random free TCP port for the public API and a second random
    free port for metrics.
  - Creates a fresh file-backed SQLite database in a temp directory and
    deletes it after the run.
  - Waits for `GET /health` to return 200 using `reqwest`.
  - Runs all operation scenarios in one `#[tokio::test]`.
  - Tears down the server.
- No external tools such as `bats` or `curl` are required.

### Server bootstrap key capture

- The server writes a JSON file to the path given by
  `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` only when the database is freshly created.
- Format: `{"platform_key": "<raw platform key>"}`.
- The harness reads this file to obtain the platform key for the scenarios.
- If the file is missing or empty, the test fails with a clear message.

### Dictionary handling

- Tests use a real public dictionary download URL for realism.
- The harness sets `RUSTSPELL_DICTIONARY_URL` to that URL.
- The server retries failed downloads with exponential backoff up to a
  configurable timeout (default 60s).
- Downloaded dictionaries are cached in a temp directory for the duration of the
  run.
- If the dictionary remains unloadable, the suite fails with a clear message.

### Test isolation

- One server process serves the entire suite.
- Scenarios run serially inside one `#[tokio::test]` to avoid state collisions.
- The SQLite database starts empty for each run.

### Test organization

- All scenarios live in `tests/live_api_tester.rs`.
- Stateless contract tests and stateful lifecycle tests are implemented as
  helper-backed blocks inside the single test function.
- Coverage is enforced by the test itself: each scenario is named after an
  `operationId` and logged; missing operations must be added explicitly.

### External deployment mode

- If `RUSTSPELL_SERVER_URL` is set, the harness skips spawning a server and runs
  scenarios against that URL.
- In external mode the caller must also provide `RUSTSPELL_PLATFORM_KEY`.
- Docker compose mode does not support external targets; it always starts both
  the server and tester services.

## Reporting

- Default output: `cargo test` output to stdout/stderr.
- `RUSTSPELL_TEST_REPORT` controls additional report formats: `console`
  (default), `json`, `junit`, `all`.
- `RUSTSPELL_TEST_REPORT_DIR` sets the output directory (default
  `target/live-test-reports/`).
- JSON report contains: run timestamp, server URL, each operationId tested,
  requested/actual status codes, pass/fail, and any schema deviations.
- JUnit report contains test cases grouped by operationId.

## Error validation

- Every non-2xx response must be checked for
  `Content-Type: application/problem+json`.
- Problem details must include `type`, `title`, `status`, and `detail`.
- `type` must match the documented `type` URI for the specific error where the
  spec/doc provides one.

## Docker support

- `Dockerfile.test`: builds the server and test runner in one image; entrypoint
  is the compiled test binary.
- `docker-compose.test.yml`: builds a `rustspell-server` service and a
  `rustspell-tester` service; the tester waits for the server to be healthy,
  then runs the live scenarios against `http://rustspell-server:3000`.
- Both artifacts must use the same temp-dir/database cleanup semantics and
  report directory handling.

## Gating

- The entire live tester is behind a Cargo feature: `live-tests`.
- Without the feature, `cargo test` skips the live tests entirely.
- With the feature enabled, the suite compiles and runs as a normal Rust test.

## Assumptions

- The server binary can write a bootstrap secrets JSON file when
  `RUSTSPELL_BOOTSTRAP_SECRETS_PATH` is set and the database is new.
- The server supports binding to `0.0.0.0:<port>` via `RUSTSPELL_PORT` and
  `RUSTSPELL_METRICS_PORT` env vars.
- A public dictionary URL is reachable from the test environment.
