#!/usr/bin/env bats

load helpers/common

setup() {
  ADMIN_KEY="$(create_admin_key)"
}

@test "listOwnOrigins returns 403 for standard key" {
  local standard_key
  standard_key="$(create_tenant_key standard)"
  run api_call "listOwnOrigins" "GET" "/tenant/origins" 403 "" "$standard_key"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 403 ]
}

@test "listOwnOrigins returns 200" {
  run api_call "listOwnOrigins" "GET" "/tenant/origins" 200 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.origins | type')" = "array" ]
}

@test "registerOrigin returns 200" {
  run api_call "registerOrigin" "POST" "/tenant/origins" 200 '{"origin":"https://app.example.com"}' "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.origin')" = "https://app.example.com" ]
}

@test "registerOrigin returns 400 for invalid body" {
  run api_call "registerOrigin" "POST" "/tenant/origins" 400 '{"origin":""}' "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 400 ]
}

@test "registerOrigin returns 403 for standard key" {
  local standard_key
  standard_key="$(create_tenant_key standard)"
  run api_call "registerOrigin" "POST" "/tenant/origins" 403 '{"origin":"https://x.example.com"}' "$standard_key"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 403 ]
}

@test "revokeOrigin returns 204" {
  origin_result="$(api_call "registerOrigin" "POST" "/tenant/origins" 200 '{"origin":"https://revoke.example.com"}' "$ADMIN_KEY")"
  origin_id="$(echo "$origin_result" | jq -r '.id')"

  run api_call "revokeOrigin" "DELETE" "/tenant/origins/$origin_id" 204 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]

  run api_call "revokeOrigin" "DELETE" "/tenant/origins/$origin_id" 404 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}

@test "revokeOrigin returns 404 for unknown id" {
  run api_call "revokeOrigin" "DELETE" "/tenant/origins/00000000-0000-0000-0000-000000000000" 404 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}
