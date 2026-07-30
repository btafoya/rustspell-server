//! HTTP middleware: CORS layer.

use std::sync::Arc;

use axum::http::Method;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use crate::store::Store;

/// Build the CORS layer from the store's per-tenant registered origins
/// (`Store::is_registered_origin`, §23.1) rather than a static allow-list —
/// origins are managed via `/tenant/origins*`, not `RUSTSPELL_CORS_ORIGINS`.
///
/// This only controls the `Access-Control-Allow-Origin` response header the
/// browser sees; it does not by itself prove the caller's own tenant owns
/// that origin — `auth::require_origin_binding` (§23.2) is the real,
/// server-side enforcement of that, applied separately in the router.
pub fn cors_layer(store: Arc<Store>) -> CorsLayer {
    CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers(Any)
        .allow_origin(AllowOrigin::predicate(move |origin, _request_parts| {
            origin
                .to_str()
                .map(|s| store.is_registered_origin(s))
                .unwrap_or(false)
        }))
}
