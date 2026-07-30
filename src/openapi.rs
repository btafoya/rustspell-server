//! Static OpenAPI 3.0 JSON document and validation helper.

/// Embedded OpenAPI 3.0 specification.
pub static OPENAPI_SPEC: &str = include_str!("../openapi.json");

/// Return the OpenAPI JSON string.
pub fn spec() -> &'static str {
    OPENAPI_SPEC
}

/// Parse the embedded spec into a typed OpenAPI 3.0 structure.
///
/// Available only in test builds to keep the public binary from depending on
/// `openapiv3`.
#[cfg(test)]
pub fn parse() -> Result<openapiv3::OpenAPI, serde_json::Error> {
    serde_json::from_str(OPENAPI_SPEC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_valid_openapi() {
        parse().expect("openapi.json should be a valid OpenAPI 3.0.3 document");
    }
}
