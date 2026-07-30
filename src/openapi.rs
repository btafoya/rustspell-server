//! Static OpenAPI 3.0 JSON document and validation helper.

/// Embedded OpenAPI 3.0 specification.
pub static OPENAPI_SPEC: &str = include_str!("../openapi.json");

/// Return the OpenAPI JSON string.
pub fn spec() -> &'static str {
    OPENAPI_SPEC
}

/// Validate that the embedded spec is valid JSON.
#[cfg(test)]
pub fn validate() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(OPENAPI_SPEC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_valid_json() {
        validate().expect("openapi.json should be valid JSON");
    }
}
