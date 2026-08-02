use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, http::Request};
use criterion::{criterion_group, criterion_main, Criterion};
use rustspell_server::{
    auth::RateLimiter,
    config::Config,
    dictionary::DictionaryManager,
    engine::{Engine, EngineRegistry},
    handlers::{build_app, AppState},
    store::{Role, Store},
    usage::UsageRecorder,
};
use serde_json::json;
use tower::ServiceExt;

async fn test_app() -> (axum::Router, String) {
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
        db_path: std::path::PathBuf::from("/tmp/rustspell-bench.db"),
        db_url: Some("sqlite::memory:".to_string()),
        auth_rate_limit_max: 10,
        auth_rate_limit_window_seconds: 60,
        auth_rate_limit_cooldown_seconds: 60,
    };
    let dictionary_manager = DictionaryManager::new(&config);
    let engines = Arc::new(EngineRegistry::new(
        config.language.clone(),
        engine,
        dictionary_manager,
    ));
    let (store, _bootstrap) = Store::open(&config).await.unwrap();
    let (tenant, _admin_key) = store
        .create_tenant("Bench Tenant".to_string(), None, None, None, None)
        .await
        .unwrap();
    let key = store
        .create_key(&tenant.id, "bench".to_string(), Role::Standard, None)
        .await
        .unwrap()
        .raw_key;
    let rate_limiter = Arc::new(RateLimiter::new(
        config.auth_rate_limit_max,
        Duration::from_secs(config.auth_rate_limit_window_seconds),
        Duration::from_secs(config.auth_rate_limit_cooldown_seconds),
    ));
    let state = Arc::new(AppState::new(
        engines,
        Arc::new(config),
        Arc::new(store),
        rate_limiter,
        Arc::new(UsageRecorder::new()),
    ));
    (build_app(state), key)
}

fn spellcheck_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (app, key) = rt.block_on(test_app());
    let body = json!({ "words": ["hello", "wrld", "hello", "wrld", "hello", "wrld"] });
    let body_bytes = axum::body::Bytes::from(body.to_string());

    c.bench_function("POST /spellcheck word list", |b| {
        b.iter(|| {
            let request = Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", &key)
                .body(Body::from(body_bytes.clone()))
                .unwrap();
            let response = rt.block_on(app.clone().oneshot(request)).unwrap();
            assert_eq!(response.status(), 200);
            rt.block_on(async {
                use http_body_util::BodyExt;
                let _bytes = response.into_body().collect().await.unwrap().to_bytes();
            });
        });
    });
}

criterion_group!(benches, spellcheck_benchmark);
criterion_main!(benches);
