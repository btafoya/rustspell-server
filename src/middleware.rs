//! HTTP middleware: CORS layer.

use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};

use crate::config::Config;

/// Build the CORS layer from the configured allow-list.
pub fn cors_layer(config: &Config) -> CorsLayer {
    CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any)
        .allow_origin(config.cors_allow_origin())
}
