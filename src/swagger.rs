//! Swagger UI documentation portal served at `/ui`.

use axum::Router;
use swagger_ui_dist::{ApiDefinition, OpenApiSource};

/// Build a router that serves the Swagger UI at `/ui` and loads the OpenAPI
/// specification from the existing `GET /docs` endpoint.
///
/// A prefix of `/` would make `swagger-ui-dist` emit asset URLs like
/// `//swagger-ui.css`, which browsers treat as protocol-relative and resolve
/// to a bogus host — hence the non-root prefix.
pub fn portal_router() -> Router {
    let api_def = ApiDefinition {
        uri_prefix: "/ui",
        api_definition: OpenApiSource::Uri("/docs"),
        title: Some("Rust Spell Server"),
    };
    swagger_ui_dist::generate_routes(api_def)
}
