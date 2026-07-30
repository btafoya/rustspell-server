//! HTTP handlers for the public API.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
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

use crate::config::Config;
use crate::engine::Engine;
use crate::error::{AppError, Result};
use crate::metrics;
use crate::middleware;
use crate::models::{
    PositionResult, PositionsResponse, SpellCheckRequest, SpellCheckResponse, TokenResult,
};

/// Shared application state.
pub struct AppState {
    pub engine: Arc<Engine>,
    pub config: Arc<Config>,
    pub start_time: Instant,
    pub request_count: AtomicU64,
}

impl AppState {
    pub fn new(engine: Arc<Engine>, config: Arc<Config>) -> Self {
        Self {
            engine,
            config,
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

/// `POST /spellcheck`
pub async fn spellcheck(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SpellCheckRequest>,
) -> Result<Json<SpellCheckResponse>> {
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let mut results = Vec::new();
    let engine = &state.engine;

    if let Some(text) = &req.text {
        for token in engine.tokenize(text) {
            results.push(token_result(engine, &token.token));
        }
    }

    if let Some(words) = &req.words {
        for word in words {
            results.push(token_result(engine, word));
        }
    }

    ::metrics::counter!("spellcheck_tokens_total").increment(results.len() as u64);

    Ok(Json(SpellCheckResponse { results }))
}

/// `POST /spellcheck/positions`
pub async fn spellcheck_positions(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SpellCheckRequest>,
) -> Result<Json<PositionsResponse>> {
    req.validate()
        .map_err(|e: validator::ValidationErrors| AppError::Validation(e))?;

    let engine = &state.engine;
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
    let cors = middleware::cors_layer(&state.config);
    // The portal router is stateless; convert it to the same missing-state type
    // as the API router so they can be merged.
    let portal: Router<Arc<AppState>> = crate::swagger::portal_router().with_state(());

    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::permanent("/ui") }),
        )
        .route("/health", get(health_check))
        .route("/docs", get(openapi_docs))
        .route("/spellcheck", post(spellcheck))
        .route("/spellcheck/positions", post(spellcheck_positions))
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

    fn test_state() -> Arc<AppState> {
        let aff = r"SET UTF-8
TRY abc
";
        let dic = r"2
hello
world
";
        let engine = Arc::new(Engine::new(aff, dic).unwrap());
        let config = Arc::new(Config {
            port: 3000,
            metrics_port: 9090,
            log_level: "info".to_string(),
            language: "en_US".to_string(),
            dictionary_url: "https://example.com".to_string(),
            dictionary_dir: std::path::PathBuf::from("/tmp"),
            refresh_interval_hours: 24,
            cors_origins: vec![],
        });
        Arc::new(AppState::new(engine, config))
    }

    #[tokio::test]
    async fn health_check_returns_ok() {
        let state = test_state();
        let res = health_check(Query(HealthQuery { verbose: None }), State(state)).await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn spellcheck_rejects_missing_input() {
        let state = test_state();
        let req = SpellCheckRequest {
            text: None,
            words: None,
        };
        let res = spellcheck(State(state), Json(req)).await;
        assert!(res.is_err());
    }
}
