#!/usr/bin/env bats

load helpers/common

setup() {
  PLATFORM_KEY="$RUSTSPELL_PLATFORM_KEY"
  TENANT_RESULT="$(api_call "createTenant" "POST" "/tenants" 200 '{"name":"Lifecycle Tenant"}' "$PLATFORM_KEY")"
  TENANT_ID="$(echo "$TENANT_RESULT" | jq -r '.id')"
  ADMIN_KEY="$(echo "$TENANT_RESULT" | jq -r '.admin_key.key')"
}

@test "getOwnTenant returns 200" {
  run api_call "getOwnTenant" "GET" "/tenant" 200 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.id')" = "$TENANT_ID" ]
}

@test "getOwnTenant returns 401 without key" {
  run api_call "getOwnTenant" "GET" "/tenant" 401
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 401 ]
}

@test "createTenant returns 200 with admin key" {
  run api_call "createTenant" "POST" "/tenants" 200 '{"name":"Create Test"}' "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ -n "$(echo "$output" | jq -r '.id')" ]
  [ -n "$(echo "$output" | jq -r '.admin_key.key')" ]
}

@test "createTenant returns 400 for invalid body" {
  run api_call "createTenant" "POST" "/tenants" 400 '{"name":""}' "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 400 ]
}

@test "createTenant returns 403 with Origin header" {
  local operation_id="createTenant"
  local method="POST"
  local path="/tenants"
  local expected_status=403
  local url="${RUSTSPELL_SERVER_URL}${path}"
  local response_file
  response_file="$(mktemp)"
  run curl -s -S -o "$response_file" -w "%{http_code}" -X POST \
    -H "X-API-Key: $PLATFORM_KEY" \
    -H "Content-Type: application/json" \
    -H "Origin: https://example.com" \
    -d '{"name":"Origin Test"}' \
    "$url"
  local actual="$output"
  [ "$status" -eq 0 ]
  [ "$actual" -eq "$expected_status" ]

  if [ -n "${RUSTSPELL_TEST_LOG_FILE:-}" ]; then
    printf '%s\n' "{\"operation_id\":\"$operation_id\",\"method\":\"$method\",\"path\":\"$path\",\"expected_status\":$expected_status,\"actual_status\":$actual,\"passed\":true}" >> "$RUSTSPELL_TEST_LOG_FILE"
  fi
  rm -f "$response_file"
}

@test "listTenants returns 200" {
  run api_call "listTenants" "GET" "/tenants" 200 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.tenants | type')" = "array" ]
}

@test "getTenant returns 200" {
  run api_call "getTenant" "GET" "/tenants/$TENANT_ID" 200 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.id')" = "$TENANT_ID" ]
}

@test "getTenant returns 404 for unknown id" {
  run api_call "getTenant" "GET" "/tenants/00000000-0000-0000-0000-000000000000" 404 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}

@test "updateTenant returns 200" {
  run api_call "updateTenant" "PATCH" "/tenants/$TENANT_ID" 200 '{"name":"Updated Tenant"}' "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.name')" = "Updated Tenant" ]
}

@test "updateTenant returns 404 for unknown id" {
  run api_call "updateTenant" "PATCH" "/tenants/00000000-0000-0000-0000-000000000000" 404 '{"name":"Nope"}' "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}

@test "suspendTenant and reactivateTenant return 204" {
  run api_call "suspendTenant" "POST" "/tenants/$TENANT_ID/suspend" 204 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]

  run api_call "getOwnTenant" "GET" "/tenant" 403 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 403 ]

  run api_call "reactivateTenant" "POST" "/tenants/$TENANT_ID/reactivate" 204 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]

  run api_call "getOwnTenant" "GET" "/tenant" 200 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
}

@test "suspendTenant returns 404 for unknown id" {
  run api_call "suspendTenant" "POST" "/tenants/00000000-0000-0000-0000-000000000000/suspend" 404 "" "$PLATFORM_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}
