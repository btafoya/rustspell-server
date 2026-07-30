#!/usr/bin/env bats

load helpers/common

setup() {
  ADMIN_KEY="$(create_admin_key)"
  STANDARD_KEY="$(create_tenant_key standard)"
}

@test "createApiKey returns 200 and a raw key" {
  run api_call "createApiKey" "POST" "/api-keys" 200 '{"label":"new-key","role":"admin"}' "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ -n "$(echo "$output" | jq -r '.key')" ]
  [ "$(echo "$output" | jq -r '.label')" = "new-key" ]
  [ "$(echo "$output" | jq -r '.role')" = "admin" ]
}

@test "createApiKey returns 400 for invalid body" {
  run api_call "createApiKey" "POST" "/api-keys" 400 '{"label":""}' "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 400 ]
}

@test "createApiKey returns 403 for standard key" {
  run api_call "createApiKey" "POST" "/api-keys" 403 '{"label":"nope","role":"admin"}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 403 ]
}

@test "listApiKeys returns 200" {
  run api_call "listApiKeys" "GET" "/api-keys" 200 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.keys | type')" = "array" ]
}

@test "listApiKeys returns 403 for standard key" {
  run api_call "listApiKeys" "GET" "/api-keys" 403 "" "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 403 ]
}

@test "revokeApiKey returns 204 and rotation invalidates the old key" {
  key_result="$(api_call "createApiKey" "POST" "/api-keys" 200 '{"label":"to-revoke","role":"standard"}' "$ADMIN_KEY")"
  key_id="$(echo "$key_result" | jq -r '.id')"

  run api_call "revokeApiKey" "DELETE" "/api-keys/$key_id" 204 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]

  run api_call "revokeApiKey" "DELETE" "/api-keys/$key_id" 204 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
}

@test "revokeApiKey returns 404 for unknown key id" {
  run api_call "revokeApiKey" "DELETE" "/api-keys/00000000-0000-0000-0000-000000000000" 404 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}

@test "rotateApiKey returns 200 with a new raw key" {
  key_result="$(api_call "createApiKey" "POST" "/api-keys" 200 '{"label":"to-rotate","role":"standard"}' "$ADMIN_KEY")"
  key_id="$(echo "$key_result" | jq -r '.id')"
  old_key="$(echo "$key_result" | jq -r '.key')"

  run api_call "rotateApiKey" "POST" "/api-keys/$key_id/rotate" 200 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  new_key="$(echo "$output" | jq -r '.key')"
  [ -n "$new_key" ]
  [ "$new_key" != "$old_key" ]
  [ "$(echo "$output" | jq -r '.id')" = "$key_id" ]
}

@test "rotateApiKey returns 404 for unknown key id" {
  run api_call "rotateApiKey" "POST" "/api-keys/00000000-0000-0000-0000-000000000000/rotate" 404 "" "$ADMIN_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 404 ]
}
