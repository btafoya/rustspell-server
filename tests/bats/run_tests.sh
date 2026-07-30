#!/usr/bin/env bash
set -euo pipefail

# Entrypoint for the standalone Docker test image.
# Starts a fresh server process, waits for it to be healthy, runs the bats
# suite, then tears the server down. If RUSTSPELL_SERVER_URL is already set,
# the script skips spawning a server and runs bats against that URL (caller
# must also set RUSTSPELL_PLATFORM_KEY).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

if [ -n "${RUSTSPELL_SERVER_URL:-}" ]; then
  : "${RUSTSPELL_PLATFORM_KEY:?RUSTSPELL_PLATFORM_KEY is required when RUSTSPELL_SERVER_URL is set}"
  echo "Running bats against external server: $RUSTSPELL_SERVER_URL"
  exec bats --timing "$SCRIPT_DIR"
fi

# Default ports inside the container.
RUSTSPELL_PORT="${RUSTSPELL_PORT:-3000}"
RUSTSPELL_METRICS_PORT="${RUSTSPELL_METRICS_PORT:-9090}"

TMP_DIR="$(mktemp -d)"
trap 'kill "$SERVER_PID" 2>/dev/null || true; rm -rf "$TMP_DIR"' EXIT

SERVER_BIN="${RUSTSPELL_SERVER_BIN:-$REPO_ROOT/target/release/rustspell-server}"
if [ ! -x "$SERVER_BIN" ]; then
  echo "Building server binary..."
  (cd "$REPO_ROOT" && cargo build --release --bin rustspell-server)
  SERVER_BIN="$REPO_ROOT/target/release/rustspell-server"
fi

export RUSTSPELL_PORT
export RUSTSPELL_METRICS_PORT
export RUSTSPELL_DB_PATH="$TMP_DIR/rustspell.db"
export RUSTSPELL_DICTIONARY_DIR="$TMP_DIR/dictionaries"
export RUSTSPELL_DICTIONARY_URL="${RUSTSPELL_DICTIONARY_URL:-https://raw.githubusercontent.com/LibreOffice/dictionaries/master/en}"
export RUSTSPELL_BOOTSTRAP_SECRETS_PATH="$TMP_DIR/bootstrap.json"
export RUSTSPELL_LOG_LEVEL="warn"

"$SERVER_BIN" &
SERVER_PID=$!

SERVER_URL="http://127.0.0.1:$RUSTSPELL_PORT"
for _ in $(seq 1 240); do
  if curl -s -o /dev/null -w "%{http_code}" --max-time 2 "$SERVER_URL/health" | grep -q '^200$'; then
    break
  fi
  sleep 0.5
done

if ! curl -s -o /dev/null -w "%{http_code}" --max-time 2 "$SERVER_URL/health" | grep -q '^200$'; then
  echo "Server failed to become healthy at $SERVER_URL" >&2
  exit 1
fi

export RUSTSPELL_SERVER_URL="$SERVER_URL"
export RUSTSPELL_PLATFORM_KEY="$(jq -r '.platform_key' "$TMP_DIR/bootstrap.json")"

bats --timing "$SCRIPT_DIR"
