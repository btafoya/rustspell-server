use std::sync::Arc;

use axum::{body::Body, http::Request};
use criterion::{criterion_group, criterion_main, Criterion};
use rustspell_server::{
    config::Config,
    engine::Engine,
    handlers::{build_app, AppState},
};
use serde_json::json;
use tower::ServiceExt;

fn test_app() -> axum::Router {
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
        cors_origins: vec![axum::http::HeaderValue::from_static(
            "http://localhost:3000",
        )],
    });
    let state = Arc::new(AppState::new(engine, config));
    build_app(state)
}

fn spellcheck_benchmark(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = test_app();
    let body = json!({ "words": ["hello", "wrld", "hello", "wrld", "hello", "wrld"] });
    let body_bytes = axum::body::Bytes::from(body.to_string());

    c.bench_function("POST /spellcheck word list", |b| {
        b.iter(|| {
            let request = Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
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
