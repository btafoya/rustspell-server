#!/usr/bin/env bats

load helpers/common

@test "getOpenApiSpec returns 200 with valid JSON" {
  run api_call "getOpenApiSpec" "GET" "/docs" 200
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.openapi')" = "3.0.3" ]
  [ "$(echo "$output" | jq -r '.info.title')" = "Rust Spell Server" ]
}
