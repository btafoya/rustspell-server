//! HTTP handlers for the public API.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{rejection::JsonRejection, Extension, Path, Query, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use validator::Validate;

use crate::auth;
use crate::config::Config;
use crate::engine::{Engine, EngineRegistry};
use crate::error::{AppError, Result};
use crate::metrics;
use crate::middleware;
use crate::models::{
    ApiKeyListResponse, ApiKeyMetadata, CreateApiKeyRequest, CreateTenantRequest,
    CreatedApiKeyResponse, CreatedTenant, OriginListResponse, OriginMetadata, PositionResult,
    PositionsResponse, RegisterOriginRequest, SpellCheckRequest, SpellCheckResponse,
    TenantListResponse, TenantMetadata, TokenResult, UpdateTenantRequest,
};
use crate::store::{KeyRecord, Store};

/// Shared application state.
pub struct AppState {
    pub engines: Arc<EngineRegistry>,
    pub config: Arc<Config>,
    pub store: Arc<Store>,
    pub rate_limiter: Arc<auth::RateLimiter>,
    pub start_time: Instant,
    pub request_count: AtomicU64,
}

impl AppState {
    pub fn new(
        engines: Arc<EngineRegistry>,
        config: Arc<Config>,
        store: Arc<Store>,
        rate_limiter: Arc<auth::RateLimiter>,
    ) -> Self {
        Self {
            engines,
            config,
            store,
            rate_limiter,
            start_time: Instant::now(),
            request_count: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct HealthQuery {
    verbose: Option<bool>,
}

/// `GET /health`
pub async fn health_check(
    Query(query): Query<HealthQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let body = if query.verbose == Some(true) {
        json!({
            "status": "ok",
            "uptime_seconds": state.start_time.elapsed().as_secs(),
            "request_count": state.request_count.load(Ordering::Relaxed),
        })
    } else {
        json!({ "status": "ok" })
    };

    (StatusCode::OK, Json(body)).into_response()
}

/// `GET /docs`
pub async fn openapi_docs(State(_state): State<Arc<AppState>>) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        crate::openapi::spec(),
    )
        .into_response()
}

/// Resolves the engine for a request: the explicit `language` override if
/// present, otherwise the calling tenant's default language. Downloads and
/// caches the language on first use if it isn't already loaded (§25);
/// `EngineError` (bad code, download/parse failure) maps to 400, not 500 —
/// only the startup-time default language gets fail-fast treatment (§5.3).
async fn resolve_engine(
    state: &AppState,
    caller: &KeyRecord,
    requested_language: Option<&str>,
) -> Result<Arc<Engine>> {
    let language = match requested_language {
        Some(language) => language.to_string(),
        None => {
            let tenant_id = caller
                .tenant_id
                .as_deref()
                .expect("require_active_tenant guarantees a tenant-scoped key");
            state
                .store
                .get_tenant(tenant_id)
                .map(|t| t.language)
                .unwrap_or_else(|| state.config.language.clone())
        }
    };

    state
        .engines
        .get_or_load(&language)
        .await
        .map_err(|e| AppError::UnsupportedLanguage(e.to_string()))
}

/// `POST /spellcheck`
pub async fn spellcheck(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    req: std::result::Result<Json<SpellCheckRequest>, JsonRejection>,
) -> Result<Json<SpellCheckResponse>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let mut results = Vec::new();
    let engine = resolve_engine(&state, &caller, req.language.as_deref()).await?;

    if let Some(text) = &req.text {
        for token in engine.tokenize(text) {
            results.push(token_result(&engine, &token.token));
        }
    }

    if let Some(words) = &req.words {
        for word in words {
            results.push(token_result(&engine, word));
        }
    }

    ::metrics::counter!("spellcheck_tokens_total").increment(results.len() as u64);

    Ok(Json(SpellCheckResponse { results }))
}

/// `POST /spellcheck/positions`
pub async fn spellcheck_positions(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    req: std::result::Result<Json<SpellCheckRequest>, JsonRejection>,
) -> Result<Json<PositionsResponse>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let engine = resolve_engine(&state, &caller, req.language.as_deref()).await?;
    let mut by_token: std::collections::HashMap<String, PositionAccumulator> =
        std::collections::HashMap::new();

    if let Some(text) = &req.text {
        for token in engine.tokenize(text) {
            if engine.check(&token.token) {
                continue;
            }
            let entry = by_token.entry(token.token.clone()).or_default();
            entry.positions.push(token.start_char);
            if entry.suggestions.is_empty() {
                entry.suggestions = engine.suggest(&token.token);
            }
        }
    }

    if let Some(words) = &req.words {
        for word in words {
            if engine.check(word) {
                continue;
            }
            let entry = by_token.entry(word.clone()).or_default();
            if entry.suggestions.is_empty() {
                entry.suggestions = engine.suggest(word);
            }
        }
    }

    let results: Vec<PositionResult> = by_token
        .into_iter()
        .map(|(token, acc)| PositionResult {
            token,
            positions: acc.positions,
            suggestions: acc.suggestions,
        })
        .collect();

    Ok(Json(PositionsResponse { results }))
}

#[derive(Default)]
struct PositionAccumulator {
    positions: Vec<usize>,
    suggestions: Vec<String>,
}

fn token_result(engine: &Engine, token: &str) -> TokenResult {
    let valid = engine.check(token);
    let suggestions = if valid {
        Vec::new()
    } else {
        engine.suggest(token)
    };
    TokenResult {
        token: token.to_string(),
        valid,
        suggestions,
    }
}

/// `POST /api-keys` (admin key, scoped to the caller's own tenant).
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    req: std::result::Result<Json<CreateApiKeyRequest>, JsonRejection>,
) -> Result<Json<CreatedApiKeyResponse>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let created = state
        .store
        .create_key(tenant_id, req.label, req.role, req.expires_at)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(CreatedApiKeyResponse {
        metadata: ApiKeyMetadata::from(&created.record),
        key: created.raw_key,
    }))
}

/// `GET /api-keys` (admin key, scoped to the caller's own tenant).
pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
) -> Result<Json<ApiKeyListResponse>> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let keys = state
        .store
        .list_keys(tenant_id)
        .iter()
        .map(ApiKeyMetadata::from)
        .collect();

    Ok(Json(ApiKeyListResponse { keys }))
}

/// `DELETE /api-keys/{id}` (admin key, scoped to the caller's own tenant).
/// Idempotent: revoking an already-revoked key still returns 204. Unknown or
/// cross-tenant `id`s return 404 (never 403 — no existence leak).
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let found = state
        .store
        .revoke_key(tenant_id, &id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// `POST /api-keys/{id}/rotate` (admin key, scoped to the caller's own tenant).
pub async fn rotate_api_key(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    Path(id): Path<String>,
) -> Result<Json<CreatedApiKeyResponse>> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let rotated = state
        .store
        .rotate_key(tenant_id, &id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    match rotated {
        Some(created) => Ok(Json(CreatedApiKeyResponse {
            metadata: ApiKeyMetadata::from(&created.record),
            key: created.raw_key,
        })),
        None => Err(AppError::NotFound),
    }
}

/// `GET /tenant` (admin or standard key, self — the one route that skips
/// `require_admin`; see `DESIGN.md` §22.2).
pub async fn get_own_tenant(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
) -> Result<Json<TenantMetadata>> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_active_tenant guarantees a tenant-scoped key");
    let tenant = state
        .store
        .get_tenant(tenant_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(TenantMetadata::from(&tenant)))
}

/// `GET /tenant/origins` (admin key, scoped to the caller's own tenant).
pub async fn list_own_origins(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
) -> Result<Json<OriginListResponse>> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");
    let origins = state
        .store
        .list_origins(tenant_id)
        .iter()
        .map(OriginMetadata::from)
        .collect();
    Ok(Json(OriginListResponse { origins }))
}

/// `POST /tenant/origins` (admin key, scoped to the caller's own tenant).
pub async fn register_origin(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    req: std::result::Result<Json<RegisterOriginRequest>, JsonRejection>,
) -> Result<Json<OriginMetadata>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let info = state
        .store
        .register_origin(tenant_id, req.origin)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(OriginMetadata::from(&info)))
}

/// `DELETE /tenant/origins/{id}` (admin key, scoped to the caller's own
/// tenant). Unknown or cross-tenant `id`s return 404, never 403.
pub async fn revoke_origin(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<KeyRecord>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let tenant_id = caller
        .tenant_id
        .as_deref()
        .expect("require_admin guarantees a tenant-scoped key");

    let found = state
        .store
        .revoke_origin(tenant_id, &id)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// `POST /tenants` (platform key). Creates a tenant and its first admin key
/// in one call.
pub async fn create_tenant(
    State(state): State<Arc<AppState>>,
    req: std::result::Result<Json<CreateTenantRequest>, JsonRejection>,
) -> Result<Json<CreatedTenant>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let (tenant, admin_key) = state
        .store
        .create_tenant(
            req.name,
            req.language,
            req.quota_limit,
            req.period_start,
            req.period_end,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(CreatedTenant {
        tenant: TenantMetadata::from(&tenant),
        admin_key: CreatedApiKeyResponse {
            metadata: ApiKeyMetadata::from(&admin_key.record),
            key: admin_key.raw_key,
        },
    }))
}

/// `GET /tenants` (platform key).
pub async fn list_tenants(State(state): State<Arc<AppState>>) -> Json<TenantListResponse> {
    let tenants = state
        .store
        .list_tenants()
        .iter()
        .map(TenantMetadata::from)
        .collect();
    Json(TenantListResponse { tenants })
}

/// `GET /tenants/{id}` (platform key).
pub async fn get_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<TenantMetadata>> {
    let tenant = state.store.get_tenant(&id).ok_or(AppError::NotFound)?;
    Ok(Json(TenantMetadata::from(&tenant)))
}

/// `PATCH /tenants/{id}` (platform key).
pub async fn update_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    req: std::result::Result<Json<UpdateTenantRequest>, JsonRejection>,
) -> Result<Json<TenantMetadata>> {
    let Json(req) = req?;
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let updated = state
        .store
        .update_tenant(
            &id,
            req.name,
            req.language,
            req.quota_limit,
            req.request_count,
            req.period_start,
            req.period_end,
        )
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;

    match updated {
        Some(tenant) => Ok(Json(TenantMetadata::from(&tenant))),
        None => Err(AppError::NotFound),
    }
}

/// `POST /tenants/{id}/suspend` (platform key).
pub async fn suspend_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let found = state
        .store
        .set_suspended(&id, true)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// `POST /tenants/{id}/reactivate` (platform key).
pub async fn reactivate_tenant(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let found = state
        .store
        .set_suspended(&id, false)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// Middleware that increments the global request counter.
pub async fn request_counter(
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    state.request_count.fetch_add(1, Ordering::Relaxed);
    next.run(request).await
}

/// Middleware that records request count, status, and latency metrics.
pub async fn metrics_middleware(request: axum::extract::Request, next: Next) -> Response {
    let start = metrics::record_request_start();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    metrics::record_request_end(&method, &path, status, start);
    response
}

/// Build the public API router, including the Swagger UI portal at `/ui`.
pub fn build_app(state: Arc<AppState>) -> Router {
    let cors = middleware::cors_layer(state.store.clone());
    // The portal router is stateless; convert it to the same missing-state type
    // as the API router so they can be merged.
    let portal: Router<Arc<AppState>> = crate::swagger::portal_router().with_state(());

    // Tower/axum layer stacking: the *last* `.route_layer()` added is the
    // outermost wrapper and runs *first* on the request path. `require_active_key`
    // must therefore be added last in every group below, since everything
    // else reads the `KeyRecord` it inserts.
    // `require_quota` is added first (runs last, closest to the handler) so
    // a request already rejected by an earlier layer (suspended tenant,
    // bad origin) never consumes quota.
    let protected_spellcheck = Router::new()
        .route("/spellcheck", post(spellcheck))
        .route("/spellcheck/positions", post(spellcheck_positions))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_quota,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_origin_binding,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_tenant,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_key,
        ));

    let api_key_routes = Router::new()
        .route("/api-keys", post(create_api_key).get(list_api_keys))
        .route("/api-keys/:id", delete(revoke_api_key))
        .route("/api-keys/:id/rotate", post(rotate_api_key))
        .route_layer(axum::middleware::from_fn(auth::require_admin))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_origin_binding,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_tenant,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_key,
        ));

    // `GET /tenant` is the one route in this area that both `admin` and
    // `standard` keys can call — it's self-service usage visibility, not a
    // mutation — so it skips `require_admin` (see `DESIGN.md` §22.2).
    let tenant_self_get = Router::new()
        .route("/tenant", get(get_own_tenant))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_origin_binding,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_tenant,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_key,
        ));

    let tenant_origin_routes = Router::new()
        .route(
            "/tenant/origins",
            get(list_own_origins).post(register_origin),
        )
        .route("/tenant/origins/:id", delete(revoke_origin))
        .route_layer(axum::middleware::from_fn(auth::require_admin))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_origin_binding,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_tenant,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_active_key,
        ));

    // `require_platform_key` subsumes key resolution itself (§23.3), so this
    // group needs no separate `require_active_key` layer.
    let platform_routes = Router::new()
        .route("/tenants", post(create_tenant).get(list_tenants))
        .route("/tenants/:id", get(get_tenant).patch(update_tenant))
        .route("/tenants/:id/suspend", post(suspend_tenant))
        .route("/tenants/:id/reactivate", post(reactivate_tenant))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_platform_key,
        ));

    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::permanent("/ui") }),
        )
        .route("/health", get(health_check))
        .route("/docs", get(openapi_docs))
        .merge(protected_spellcheck)
        .merge(api_key_routes)
        .merge(tenant_self_get)
        .merge(tenant_origin_routes)
        .merge(platform_routes)
        .merge(portal)
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(PropagateRequestIdLayer::x_request_id())
                .layer(TraceLayer::new_for_http())
                .layer(cors)
                .into_inner(),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            request_counter,
        ))
        .layer(axum::middleware::from_fn(metrics_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::engine::Engine;

    async fn test_state() -> Arc<AppState> {
        let aff = r"SET UTF-8
TRY abc
";
        let dic = r"2
hello
world
";
        let engine = Engine::new(aff, dic).unwrap();
        let config = Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: std::path::PathBuf::from("/tmp"),
            refresh_interval_hours: 24,
            db_path: std::path::PathBuf::from("/tmp/rustspell-handlers-test.db"),
            db_url: Some("sqlite::memory:".to_string()),
            auth_rate_limit_max: 10,
            auth_rate_limit_window_seconds: 60,
            auth_rate_limit_cooldown_seconds: 60,
        };
        let dictionary_manager = crate::dictionary::DictionaryManager::new(&config);
        let engines = Arc::new(crate::engine::EngineRegistry::new(
            config.language.clone(),
            engine,
            dictionary_manager,
        ));
        let (store, _bootstrap) = crate::store::Store::open(&config).await.unwrap();
        let rate_limiter = Arc::new(auth::RateLimiter::new(
            config.auth_rate_limit_max,
            std::time::Duration::from_secs(config.auth_rate_limit_window_seconds),
            std::time::Duration::from_secs(config.auth_rate_limit_cooldown_seconds),
        ));
        Arc::new(AppState::new(
            engines,
            Arc::new(config),
            Arc::new(store),
            rate_limiter,
        ))
    }

    #[tokio::test]
    async fn health_check_returns_ok() {
        let state = test_state().await;
        let res = health_check(Query(HealthQuery { verbose: None }), State(state)).await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn spellcheck_rejects_missing_input() {
        let state = test_state().await;
        let req = SpellCheckRequest {
            text: None,
            words: None,
            language: None,
        };
        // No tenant is actually registered for this fake KeyRecord;
        // `resolve_engine` falls back to `state.config.language` when the
        // tenant lookup misses, so this is fine for a validation-only test.
        let caller = KeyRecord {
            id: "test-key".to_string(),
            tenant_id: Some("test-tenant".to_string()),
            label: "test".to_string(),
            role: crate::store::Role::Standard,
            key_hash: "unused".to_string(),
            created_at: 0,
            expires_at: None,
            last_used_at: None,
            revoked_at: None,
        };
        let res = spellcheck(State(state), Extension(caller), Ok(Json(req))).await;
        assert!(res.is_err());
    }
}
