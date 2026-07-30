#!/usr/bin/env bats

load helpers/common

setup() {
  STANDARD_KEY="$(create_tenant_key standard)"
}

@test "spellcheck returns 200 for known words" {
  run api_call "spellcheck" "POST" "/spellcheck" 200 '{"text":"hello world"}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq '.results | length')" -eq 2 ]
  [ "$(echo "$output" | jq -r '.results[0].token')" = "hello" ]
  [ "$(echo "$output" | jq -r '.results[0].valid')" = "true" ]
  [ "$(echo "$output" | jq -r '.results[0].suggestions | length')" -eq 0 ]
}

@test "spellcheck returns 200 for explicit words array" {
  run api_call "spellcheck" "POST" "/spellcheck" 200 '{"words":["hello","helo"]}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq '.results | length')" -eq 2 ]
  [ "$(echo "$output" | jq -r '.results[0].valid')" = "true" ]
  [ "$(echo "$output" | jq -r '.results[1].valid')" = "false" ]
}

@test "spellcheck returns 400 when text and words are missing" {
  run api_call "spellcheck" "POST" "/spellcheck" 400 '{}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 400 ]
}

@test "spellcheck returns 401 without an API key" {
  run api_call "spellcheck" "POST" "/spellcheck" 401 '{"text":"hello"}'
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 401 ]
}

@test "spellcheckPositions returns 200 with positions" {
  run api_call "spellcheckPositions" "POST" "/spellcheck/positions" 200 '{"text":"helo world helo"}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq '.results | length')" -eq 1 ]
  [ "$(echo "$output" | jq -r '.results[0].token')" = "helo" ]
  [ "$(echo "$output" | jq -r '.results[0].positions | length')" -eq 2 ]
}

@test "spellcheckPositions returns 400 when text and words are missing" {
  run api_call "spellcheckPositions" "POST" "/spellcheck/positions" 400 '{}' "$STANDARD_KEY"
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 400 ]
}

@test "spellcheckPositions returns 401 without an API key" {
  run api_call "spellcheckPositions" "POST" "/spellcheck/positions" 401 '{"text":"hello"}'
  [ "$status" -eq 0 ]
  [ "$(echo "$output" | jq -r '.status')" -eq 401 ]
}
