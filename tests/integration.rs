use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderValue, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use rustspell_server::{
    config::Config,
    engine::Engine,
    handlers::{self, AppState},
};
use serde_json::json;
use tower::ServiceExt;

fn test_app() -> Router {
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
        cors_origins: vec![HeaderValue::from_static("http://localhost:3000")],
    });
    let state = Arc::new(AppState::new(engine, config));
    handlers::build_app(state)
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body should be valid JSON")
}

#[tokio::test]
async fn root_redirects_to_swagger_ui() {
    let app = test_app();
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/ui",
        "root should redirect to the Swagger UI portal"
    );
}

#[tokio::test]
async fn ui_returns_swagger_ui_html() {
    let app = test_app();
    let response = app
        .oneshot(Request::builder().uri("/ui").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("text/html"));

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("/docs"),
        "Swagger UI page should load the OpenAPI spec from /docs"
    );
    assert!(
        !body.contains("\"//"),
        "asset URLs must not be protocol-relative"
    );
}

#[tokio::test]
async fn health_returns_ok() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn verbose_health_includes_request_count() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/health?verbose=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["status"], "ok");
    assert!(body.get("uptime_seconds").is_some());
    assert!(body.get("request_count").is_some());
}

#[tokio::test]
async fn docs_returns_openapi_spec() {
    let app = test_app();
    let response = app
        .oneshot(Request::builder().uri("/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["openapi"], "3.0.3");
    assert!(body.get("paths").is_some());
}

#[tokio::test]
async fn spellcheck_text_returns_per_token_results() {
    let app = test_app();
    let body = json!({ "text": "hello wrld" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["token"], "hello");
    assert_eq!(results[0]["valid"], true);
    assert_eq!(results[1]["token"], "wrld");
    assert_eq!(results[1]["valid"], false);
    assert!(results[1]["suggestions"]
        .as_array()
        .unwrap()
        .contains(&json!("world")));
}

#[tokio::test]
async fn spellcheck_words_returns_results() {
    let app = test_app();
    let body = json!({ "words": ["hello", "wrld"] });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn spellcheck_rejects_missing_input() {
    let app = test_app();
    let body = json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["status"], 400);
    assert_eq!(body["title"], "Validation error");
}

#[tokio::test]
async fn spellcheck_positions_returns_misspelled_positions() {
    let app = test_app();
    let body = json!({ "text": "hello wrld hello wrld" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck/positions")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["token"], "wrld");
    let positions = results[0]["positions"].as_array().unwrap();
    assert_eq!(positions.len(), 2);
}

#[tokio::test]
async fn openapi_spec_covers_all_public_paths() {
    let spec = rustspell_server::openapi::OPENAPI_SPEC;
    let doc: serde_json::Value = serde_json::from_str(spec).unwrap();
    let paths = doc["paths"].as_object().unwrap();

    for route in ["/health", "/docs", "/spellcheck", "/spellcheck/positions"] {
        assert!(
            paths.contains_key(route),
            "openapi.json is missing path {route}"
        );
    }
}

#[tokio::test]
async fn openapi_operations_declare_runtime_status_codes() {
    let spec = rustspell_server::openapi::OPENAPI_SPEC;
    let doc: serde_json::Value = serde_json::from_str(spec).unwrap();

    let operations: Vec<(&str, &str, &str, &[&str])> = vec![
        ("/health", "get", "healthCheck", &["200", "500"]),
        ("/docs", "get", "getOpenApiSpec", &["200", "500"]),
        ("/spellcheck", "post", "spellcheck", &["200", "400", "500"]),
        (
            "/spellcheck/positions",
            "post",
            "spellcheckPositions",
            &["200", "400", "500"],
        ),
    ];

    for (path, method, expected_operation_id, expected_statuses) in operations {
        let op = &doc["paths"][path][method];
        assert_eq!(
            op["operationId"].as_str().unwrap(),
            expected_operation_id,
            "operationId mismatch for {method} {path}"
        );
        let responses = op["responses"].as_object().unwrap();
        for status in expected_statuses {
            assert!(
                responses.contains_key(*status),
                "{method} {path} is missing response {status}"
            );
        }
    }
}

#[tokio::test]
async fn cors_allows_configured_origin() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/spellcheck")
                .header("origin", "http://localhost:3000")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(allow_origin, "http://localhost:3000");
}

#[tokio::test]
async fn cors_blocks_unconfigured_origin() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/spellcheck")
                .header("origin", "https://evil.com")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}
