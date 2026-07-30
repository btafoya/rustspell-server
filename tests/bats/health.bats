#!/usr/bin/env bats

load helpers/common

@test "healthCheck returns 200" {
  run api_call "healthCheck" "GET" "/health" 200
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" = "ok" ]
}

@test "healthCheck verbose returns 200 with extra fields" {
  run api_call "healthCheck" "GET" "/health?verbose=true" 200
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" = "ok" ]
  [ "$(echo "$output" | jq -r '.uptime_seconds')" != "null" ]
  [ "$(echo "$output" | jq -r '.request_count')" != "null" ]
}
