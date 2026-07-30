#!/usr/bin/env bash
set -euo pipefail

# Entrypoint for the docker-compose.test.yml tester service. The server writes
# its bootstrap platform key to a shared file; this script reads it and runs
# bats against the configured RUSTSPELL_SERVER_URL.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

: "${RUSTSPELL_SERVER_URL:?RUSTSPELL_SERVER_URL is required}"
: "${RUSTSPELL_PLATFORM_KEY_FILE:?RUSTSPELL_PLATFORM_KEY_FILE is required}"

export RUSTSPELL_PLATFORM_KEY
RUSTSPELL_PLATFORM_KEY="$(jq -r '.platform_key' "$RUSTSPELL_PLATFORM_KEY_FILE")"

bats --timing "$SCRIPT_DIR"
