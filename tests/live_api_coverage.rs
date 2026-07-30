//! Coverage check: every operationId declared in `openapi.json` must be listed
//! in the bats manifest (`tests/bats/MANIFEST.json`).

#![cfg(feature = "live-tests")]

use std::collections::HashSet;

use serde_json::Value;

#[test]
fn openapi_operations_have_live_tests() {
    let spec = include_str!("../openapi.json");
    let doc: Value = serde_json::from_str(spec).expect("openapi.json is valid JSON");

    let manifest = include_str!("bats/MANIFEST.json");
    let manifest: Value = serde_json::from_str(manifest).expect("MANIFEST.json is valid JSON");

    let spec_ops: HashSet<String> = doc["paths"]
        .as_object()
        .expect("openapi.json has paths")
        .values()
        .flat_map(|methods| methods.as_object().expect("path has methods").values())
        .filter_map(|op| op["operationId"].as_str().map(String::from))
        .collect();

    let covered_ops: HashSet<String> = manifest["covered_operations"]
        .as_array()
        .expect("MANIFEST.json has covered_operations")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let mut missing: Vec<String> = spec_ops.difference(&covered_ops).cloned().collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "operationIds missing live tests: {:?}",
        missing
    );
}
