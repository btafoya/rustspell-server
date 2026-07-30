#!/usr/bin/env bash

# Make an HTTP call, log the result, and assert the status code.
#
# Usage: api_call <operation_id> <method> <path> <expected_status> [body] [api_key]
#
# The response body is echoed to stdout. If RUSTSPELL_TEST_LOG_FILE is set, a
# JSON line describing the call is appended to it.
api_call() {
  local operation_id="$1"
  local method="$2"
  local path="$3"
  local expected_status="$4"
  local body="${5:-}"
  local api_key="${6:-}"

  local url="${RUSTSPELL_SERVER_URL}${path}"
  local response_file
  response_file="$(mktemp)"

  local curl_args=(-s -S -o "$response_file" -w "%{http_code}" --max-time 30)

  if [ -n "$api_key" ]; then
    curl_args+=(-H "X-API-Key: $api_key")
  fi

  curl_args+=(-X "$method" -H "Content-Type: application/json")

  if [ -n "$body" ]; then
    curl_args+=(-d "$body")
  fi

  curl_args+=("$url")

  local status_code
  status_code="$(curl "${curl_args[@]}")"

  local body_text
  body_text="$(cat "$response_file")"
  rm -f "$response_file"

  local passed="true"
  if [ "$status_code" != "$expected_status" ]; then
    passed="false"
  fi

  if [ -n "${RUSTSPELL_TEST_LOG_FILE:-}" ]; then
    printf '%s\n' "{\"operation_id\":\"$operation_id\",\"method\":\"$method\",\"path\":\"$path\",\"expected_status\":$expected_status,\"actual_status\":$status_code,\"passed\":$passed}" >> "$RUSTSPELL_TEST_LOG_FILE"
  fi

  if [ "$passed" != "true" ]; then
    echo "Expected $expected_status but got $status_code for $operation_id $method $path" >&2
  fi

  [ "$passed" = "true" ]
  echo "$body_text"
}

# Create a tenant using the platform key and return the admin key.
create_admin_key() {
  local result
  result="$(api_call "createTenant" "POST" "/tenants" 200 '{"name":"Live Test Tenant"}' "$RUSTSPELL_PLATFORM_KEY")"
  echo "$result" | jq -r '.admin_key.key'
}

# Create a tenant and a key of the requested role (admin or standard).
create_tenant_key() {
  local role="$1"
  local result
  result="$(api_call "createTenant" "POST" "/tenants" 200 '{"name":"Live Test Tenant"}' "$RUSTSPELL_PLATFORM_KEY")"
  local admin_key
  admin_key="$(echo "$result" | jq -r '.admin_key.key')"

  if [ "$role" = "admin" ]; then
    echo "$admin_key"
    return
  fi

  local key_result
  key_result="$(api_call "createApiKey" "POST" "/api-keys" 200 "{\"label\":\"standard-key\",\"role\":\"standard\"}" "$admin_key")"
  echo "$key_result" | jq -r '.key'
}
