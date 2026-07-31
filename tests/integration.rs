use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use rustspell_server::{
    auth::RateLimiter,
    config::Config,
    dictionary::DictionaryManager,
    engine::{Engine, EngineRegistry},
    handlers::{self, AppState},
    store::{Role, Store},
};
use serde_json::json;
use tower::ServiceExt;

/// Returns the app, the store (for minting keys/tenants/origins directly),
/// and the bootstrap platform key.
async fn test_app() -> (Router, Arc<Store>, String) {
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
        db_path: std::path::PathBuf::from("/tmp/rustspell-integration.db"),
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
    let (store, bootstrap) = Store::open(&config).await.unwrap();
    let store = Arc::new(store);
    let platform_key = bootstrap
        .expect("fresh store bootstraps a platform key")
        .raw_key;
    let rate_limiter = Arc::new(RateLimiter::new(
        config.auth_rate_limit_max,
        Duration::from_secs(config.auth_rate_limit_window_seconds),
        Duration::from_secs(config.auth_rate_limit_cooldown_seconds),
    ));
    let state = Arc::new(AppState::new(
        engines,
        Arc::new(config),
        store.clone(),
        rate_limiter,
    ));
    (handlers::build_app(state), store, platform_key)
}

/// Creates a fresh tenant and returns a raw `standard`-role key for it.
async fn mint_standard_key(store: &Store) -> String {
    let (tenant, _admin_key) = store
        .create_tenant("Test Tenant".to_string(), None, None, None, None)
        .await
        .unwrap();
    store
        .create_key(
            &tenant.id,
            "test-standard".to_string(),
            Role::Standard,
            None,
        )
        .await
        .unwrap()
        .raw_key
}

/// Creates a fresh tenant and returns `(tenant_id, raw admin key)`.
async fn mint_admin_key(store: &Store) -> (String, String) {
    let (tenant, admin_key) = store
        .create_tenant("Test Tenant".to_string(), None, None, None, None)
        .await
        .unwrap();
    (tenant.id, admin_key.raw_key)
}

/// Like `test_app`, but with a caller-controlled `dictionary_dir` so tests
/// can pre-populate a second language's cache files and load it without
/// touching the network.
async fn test_app_with_dictionary_dir(
    dictionary_dir: std::path::PathBuf,
) -> (Router, Arc<Store>, String) {
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
        dictionary_dir,
        refresh_interval_hours: 24,
        db_path: std::path::PathBuf::from("/tmp/rustspell-integration-lang.db"),
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
    let (store, bootstrap) = Store::open(&config).await.unwrap();
    let store = Arc::new(store);
    let platform_key = bootstrap
        .expect("fresh store bootstraps a platform key")
        .raw_key;
    let rate_limiter = Arc::new(RateLimiter::new(
        config.auth_rate_limit_max,
        Duration::from_secs(config.auth_rate_limit_window_seconds),
        Duration::from_secs(config.auth_rate_limit_cooldown_seconds),
    ));
    let state = Arc::new(AppState::new(
        engines,
        Arc::new(config),
        store.clone(),
        rate_limiter,
    ));
    (handlers::build_app(state), store, platform_key)
}

/// Writes a fixture `.aff`/`.dic` pair at the cache path
/// `DictionaryManager::ensure_dictionary` expects for `language`.
fn write_cached_dictionary_fixture(dictionary_dir: &std::path::Path, language: &str) {
    let lang_dir = dictionary_dir.join(language);
    std::fs::create_dir_all(&lang_dir).unwrap();
    std::fs::write(
        lang_dir.join(format!("{language}.aff")),
        "SET UTF-8\nTRY abc\n",
    )
    .unwrap();
    std::fs::write(lang_dir.join(format!("{language}.dic")), "1\nbonjour\n").unwrap();
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body should be valid JSON")
}

#[tokio::test]
async fn root_redirects_to_swagger_ui() {
    let (app, _store, _platform_key) = test_app().await;
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
    let (app, _store, _platform_key) = test_app().await;
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
    let (app, _store, _platform_key) = test_app().await;
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
    let (app, _store, _platform_key) = test_app().await;
    let response = app
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
    let (app, _store, _platform_key) = test_app().await;
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
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let body = json!({ "text": "hello wrld" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
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
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let body = json!({ "words": ["hello", "wrld"] });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
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
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let body = json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
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
async fn malformed_json_body_returns_bad_request() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_id, admin_key) = mint_admin_key(&store).await;
    let body = json!({});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api-keys")
                .header("content-type", "application/json")
                .header("x-api-key", admin_key)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["status"], 400);
    assert_eq!(body["title"], "Invalid JSON");
}

#[tokio::test]
async fn spellcheck_positions_returns_misspelled_positions() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let body = json!({ "text": "hello wrld hello wrld" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck/positions")
                .header("content-type", "application/json")
                .header("x-api-key", key)
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
async fn spellcheck_without_key_returns_unauthorized() {
    let (app, _store, _platform_key) = test_app().await;
    let body = json!({ "text": "hello" });
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

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn spellcheck_with_invalid_key_returns_unauthorized() {
    let (app, _store, _platform_key) = test_app().await;
    let body = json!({ "text": "hello" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", "rsk_not-a-real-key")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn spellcheck_with_revoked_key_returns_unauthorized() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant, _admin) = store
        .create_tenant("Revoke Test".to_string(), None, None, None, None)
        .await
        .unwrap();
    let created = store
        .create_key(&tenant.id, "to-revoke".to_string(), Role::Standard, None)
        .await
        .unwrap();
    store
        .revoke_key(&tenant.id, &created.record.id)
        .await
        .unwrap();

    let body = json!({ "text": "hello" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", created.raw_key)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn standard_key_forbidden_from_api_keys() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api-keys")
                .header("x-api-key", key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_key_can_create_list_revoke_own_keys() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_id, admin_key) = mint_admin_key(&store).await;

    let create_body = json!({ "label": "ci", "role": "standard" });
    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api-keys")
                .header("content-type", "application/json")
                .header("x-api-key", &admin_key)
                .body(Body::from(create_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::OK);
    let created = body_json(create_response).await;
    let new_key_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["role"], "standard");
    assert!(created["key"].as_str().unwrap().starts_with("rsk_"));

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api-keys")
                .header("x-api-key", &admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = body_json(list_response).await;
    let keys = list_body["keys"].as_array().unwrap();
    assert!(keys.iter().any(|k| k["id"] == new_key_id));

    let revoke_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api-keys/{new_key_id}"))
                .header("x-api-key", &admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke_response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn admin_key_gets_not_found_for_other_tenants_key() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_a, admin_a) = mint_admin_key(&store).await;
    let (tenant_b, _admin_b) = store
        .create_tenant("Tenant B".to_string(), None, None, None, None)
        .await
        .unwrap();
    let key_b = store
        .create_key(&tenant_b.id, "b-key".to_string(), Role::Standard, None)
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api-keys/{}", key_b.record.id))
                .header("x-api-key", admin_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn platform_key_creates_tenant_with_admin_key() {
    let (app, _store, platform_key) = test_app().await;
    let body = json!({ "name": "Acme" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/tenants")
                .header("content-type", "application/json")
                .header("x-api-key", platform_key)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["name"], "Acme");
    assert_eq!(
        body["quota_limit"], 0,
        "omitted quota defaults to unlimited"
    );
    assert!(body["admin_key"]["key"]
        .as_str()
        .unwrap()
        .starts_with("rsk_"));
    assert_eq!(body["admin_key"]["role"], "admin");
}

#[tokio::test]
async fn platform_key_with_origin_header_is_forbidden() {
    let (app, store, platform_key) = test_app().await;
    // Register a *real* origin for a *real* tenant first — proving F43a
    // rejects the platform key's Origin header unconditionally, not just
    // because the origin happened to be unrecognized.
    let (tenant_id, _admin_key) = mint_admin_key(&store).await;
    store
        .register_origin(&tenant_id, "https://billing.example.com".to_string())
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/tenants")
                .header("x-api-key", platform_key)
                .header("origin", "https://billing.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_platform_key_forbidden_from_tenants() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/tenants")
                .header("x-api-key", key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_and_standard_keys_can_read_own_tenant() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant_id, admin_key) = mint_admin_key(&store).await;
    let standard_key = store
        .create_key(&tenant_id, "std".to_string(), Role::Standard, None)
        .await
        .unwrap()
        .raw_key;

    for key in [admin_key, standard_key] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .header("x-api-key", key)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn admin_key_can_register_list_revoke_own_origins() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_id, admin_key) = mint_admin_key(&store).await;

    let register_body = json!({ "origin": "https://app.example.com" });
    let register_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/tenant/origins")
                .header("content-type", "application/json")
                .header("x-api-key", &admin_key)
                .body(Body::from(register_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(register_response.status(), StatusCode::OK);
    let registered = body_json(register_response).await;
    let origin_id = registered["id"].as_str().unwrap().to_string();

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/tenant/origins")
                .header("x-api-key", &admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = body_json(list_response).await;
    let origins = list_body["origins"].as_array().unwrap();
    assert!(origins.iter().any(|o| o["id"] == origin_id));

    let revoke_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/tenant/origins/{origin_id}"))
                .header("x-api-key", &admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoke_response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn admin_key_gets_not_found_for_other_tenants_origin() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_a, admin_a) = mint_admin_key(&store).await;
    let (tenant_b, _admin_b) = store
        .create_tenant("Tenant B".to_string(), None, None, None, None)
        .await
        .unwrap();
    let origin_b = store
        .register_origin(&tenant_b.id, "https://b.example.com".to_string())
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/tenant/origins/{}", origin_b.id))
                .header("x-api-key", admin_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn origin_binding_allows_own_registered_origin() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant_id, admin_key) = mint_admin_key(&store).await;
    store
        .register_origin(&tenant_id, "https://app.example.com".to_string())
        .await
        .unwrap();

    let body = json!({ "text": "hello" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", admin_key)
                .header("origin", "https://app.example.com")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn origin_binding_rejects_unregistered_origin() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_id, admin_key) = mint_admin_key(&store).await;

    let body = json!({ "text": "hello" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", admin_key)
                .header("origin", "https://never-registered.example.com")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn origin_binding_rejects_a_different_tenants_registered_origin() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_a, admin_a) = mint_admin_key(&store).await;
    let (tenant_b, _admin_b) = store
        .create_tenant("Tenant B".to_string(), None, None, None, None)
        .await
        .unwrap();
    // Registered — but to tenant B, not tenant A. The CORS layer would still
    // allow this origin (it's known to *some* tenant); origin binding must
    // reject it anyway, since it's not *this* key's own tenant's origin.
    store
        .register_origin(&tenant_b.id, "https://b-only.example.com".to_string())
        .await
        .unwrap();

    let body = json!({ "text": "hello" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", admin_a)
                .header("origin", "https://b-only.example.com")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn suspended_tenant_key_forbidden_from_spellcheck_and_tenant_self() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant_id, admin_key) = mint_admin_key(&store).await;
    store.set_suspended(&tenant_id, true).await.unwrap();

    let spellcheck_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", &admin_key)
                .body(Body::from(json!({ "text": "hello" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(spellcheck_response.status(), StatusCode::FORBIDDEN);

    let tenant_self_response = app
        .oneshot(
            Request::builder()
                .uri("/tenant")
                .header("x-api-key", admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tenant_self_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn platform_key_updates_and_suspends_reactivates_tenant() {
    let (app, store, platform_key) = test_app().await;
    let (tenant, _admin_key) = store
        .create_tenant("Acme".to_string(), None, None, None, None)
        .await
        .unwrap();

    let update_body = json!({ "quota_limit": 1000 });
    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/tenants/{}", tenant.id))
                .header("content-type", "application/json")
                .header("x-api-key", &platform_key)
                .body(Body::from(update_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update_response.status(), StatusCode::OK);
    let updated = body_json(update_response).await;
    assert_eq!(updated["quota_limit"], 1000);

    let suspend_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/tenants/{}/suspend", tenant.id))
                .header("x-api-key", &platform_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(suspend_response.status(), StatusCode::NO_CONTENT);
    assert!(store.get_tenant(&tenant.id).unwrap().suspended_at.is_some());

    let reactivate_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/tenants/{}/reactivate", tenant.id))
                .header("x-api-key", platform_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reactivate_response.status(), StatusCode::NO_CONTENT);
    assert!(store.get_tenant(&tenant.id).unwrap().suspended_at.is_none());
}

#[tokio::test]
async fn openapi_spec_covers_all_public_paths() {
    let spec = rustspell_server::openapi::OPENAPI_SPEC;
    let doc: serde_json::Value = serde_json::from_str(spec).unwrap();
    let paths = doc["paths"].as_object().unwrap();

    for route in [
        "/health",
        "/docs",
        "/spellcheck",
        "/spellcheck/positions",
        "/api-keys",
        "/api-keys/{id}",
        "/api-keys/{id}/rotate",
        "/tenant",
        "/tenant/origins",
        "/tenant/origins/{id}",
        "/tenants",
        "/tenants/{id}",
        "/tenants/{id}/suspend",
        "/tenants/{id}/reactivate",
    ] {
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
        (
            "/spellcheck",
            "post",
            "spellcheck",
            &["200", "400", "401", "429", "500"],
        ),
        (
            "/spellcheck/positions",
            "post",
            "spellcheckPositions",
            &["200", "400", "401", "429", "500"],
        ),
        (
            "/api-keys",
            "post",
            "createApiKey",
            &["200", "400", "401", "403", "429"],
        ),
        ("/api-keys", "get", "listApiKeys", &["200", "401", "403"]),
        (
            "/api-keys/{id}",
            "delete",
            "revokeApiKey",
            &["204", "401", "403", "404"],
        ),
        (
            "/api-keys/{id}/rotate",
            "post",
            "rotateApiKey",
            &["200", "401", "403", "404"],
        ),
        ("/tenant", "get", "getOwnTenant", &["200", "401", "403"]),
        (
            "/tenant/origins",
            "get",
            "listOwnOrigins",
            &["200", "401", "403"],
        ),
        (
            "/tenant/origins",
            "post",
            "registerOrigin",
            &["200", "400", "401", "403"],
        ),
        (
            "/tenant/origins/{id}",
            "delete",
            "revokeOrigin",
            &["204", "401", "403", "404"],
        ),
        (
            "/tenants",
            "post",
            "createTenant",
            &["200", "400", "401", "403"],
        ),
        ("/tenants", "get", "listTenants", &["200", "401", "403"]),
        (
            "/tenants/{id}",
            "get",
            "getTenant",
            &["200", "401", "403", "404"],
        ),
        (
            "/tenants/{id}",
            "patch",
            "updateTenant",
            &["200", "400", "401", "403", "404"],
        ),
        (
            "/tenants/{id}/suspend",
            "post",
            "suspendTenant",
            &["204", "401", "403", "404"],
        ),
        (
            "/tenants/{id}/reactivate",
            "post",
            "reactivateTenant",
            &["204", "401", "403", "404"],
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
async fn cors_allows_registered_origin() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant_id, _admin_key) = mint_admin_key(&store).await;
    store
        .register_origin(&tenant_id, "http://localhost:3000".to_string())
        .await
        .unwrap();

    let response = app
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
async fn cors_blocks_unregistered_origin() {
    let (app, _store, _platform_key) = test_app().await;
    let response = app
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

#[tokio::test]
async fn spellcheck_over_quota_returns_too_many_requests() {
    let (app, store, _platform_key) = test_app().await;
    let (tenant, admin_key) = store
        .create_tenant("Quota Test".to_string(), None, Some(1), None, None)
        .await
        .unwrap();
    let key = store
        .create_key(&tenant.id, "std".to_string(), Role::Standard, None)
        .await
        .unwrap()
        .raw_key;
    let _ = admin_key; // unused beyond tenant creation

    let request = || {
        Request::builder()
            .method("POST")
            .uri("/spellcheck")
            .header("content-type", "application/json")
            .header("x-api-key", &key)
            .body(Body::from(json!({ "text": "hello" }).to_string()))
            .unwrap()
    };

    let first = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = app.oneshot(request()).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = body_json(second).await;
    assert!(
        body["type"].as_str().unwrap().contains("quota-exceeded"),
        "should be quota-exceeded, not rate-limited: {body}"
    );
}

#[tokio::test]
async fn spellcheck_quota_zero_never_blocks() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await; // default tenant has quota_limit 0 (unlimited)

    for _ in 0..5 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/spellcheck")
                    .header("content-type", "application/json")
                    .header("x-api-key", &key)
                    .body(Body::from(json!({ "text": "hello" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn platform_patch_raising_quota_unblocks_spellcheck() {
    let (app, store, platform_key) = test_app().await;
    let (tenant, _admin_key) = store
        .create_tenant("Quota Test".to_string(), None, Some(1), None, None)
        .await
        .unwrap();
    let key = store
        .create_key(&tenant.id, "std".to_string(), Role::Standard, None)
        .await
        .unwrap()
        .raw_key;

    let spellcheck = || {
        Request::builder()
            .method("POST")
            .uri("/spellcheck")
            .header("content-type", "application/json")
            .header("x-api-key", &key)
            .body(Body::from(json!({ "text": "hello" }).to_string()))
            .unwrap()
    };

    assert_eq!(
        app.clone().oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone().oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    let patch_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/tenants/{}", tenant.id))
                .header("content-type", "application/json")
                .header("x-api-key", &platform_key)
                .body(Body::from(json!({ "quota_limit": 10 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_response.status(), StatusCode::OK);

    assert_eq!(
        app.oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::OK,
        "raising quota_limit via PATCH should unblock further requests"
    );
}

#[tokio::test]
async fn platform_patch_resetting_request_count_unblocks_spellcheck() {
    let (app, store, platform_key) = test_app().await;
    let (tenant, _admin_key) = store
        .create_tenant("Quota Test".to_string(), None, Some(1), None, None)
        .await
        .unwrap();
    let key = store
        .create_key(&tenant.id, "std".to_string(), Role::Standard, None)
        .await
        .unwrap()
        .raw_key;

    let spellcheck = || {
        Request::builder()
            .method("POST")
            .uri("/spellcheck")
            .header("content-type", "application/json")
            .header("x-api-key", &key)
            .body(Body::from(json!({ "text": "hello" }).to_string()))
            .unwrap()
    };

    assert_eq!(
        app.clone().oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone().oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    // Same quota_limit, but request_count reset to 0 — simulates a billing
    // period rollover (F46): the server never does this automatically.
    let patch_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/tenants/{}", tenant.id))
                .header("content-type", "application/json")
                .header("x-api-key", &platform_key)
                .body(Body::from(json!({ "request_count": 0 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_response.status(), StatusCode::OK);

    assert_eq!(
        app.oneshot(spellcheck()).await.unwrap().status(),
        StatusCode::OK,
        "resetting request_count via PATCH should unblock further requests"
    );
}

#[tokio::test]
async fn spellcheck_without_language_uses_tenant_default() {
    // Regression: omitting `language` must behave exactly as before Stage 5.
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
                .body(Body::from(json!({ "text": "hello wrld" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["results"][0]["token"], "hello");
    assert_eq!(body["results"][0]["valid"], true);
}

#[tokio::test]
async fn spellcheck_with_language_override_loads_second_language() {
    let dir = tempfile::tempdir().unwrap();
    write_cached_dictionary_fixture(dir.path(), "fr_FR");
    let (app, store, _platform_key) = test_app_with_dictionary_dir(dir.path().to_path_buf()).await;
    let key = mint_standard_key(&store).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
                .body(Body::from(
                    json!({ "words": ["bonjour", "hello"], "language": "fr_FR" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let results = body["results"].as_array().unwrap();
    // The fr_FR fixture dictionary only knows "bonjour" — proves the
    // *override* engine was used, not the en_US default (which knows
    // "hello" but not "bonjour").
    assert_eq!(results[0]["token"], "bonjour");
    assert_eq!(results[0]["valid"], true);
    assert_eq!(results[1]["token"], "hello");
    assert_eq!(results[1]["valid"], false);
}

#[tokio::test]
async fn spellcheck_with_malformed_language_returns_bad_request() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
                .body(Body::from(
                    json!({ "text": "hello", "language": "../../etc/passwd" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn spellcheck_with_unloadable_language_returns_bad_request_not_server_error() {
    let (app, store, _platform_key) = test_app().await;
    let key = mint_standard_key(&store).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", key)
                .body(Body::from(
                    json!({ "text": "hello", "language": "xx_NOPE" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert!(body["type"]
        .as_str()
        .unwrap()
        .contains("unsupported-language"));
}

#[tokio::test]
async fn admin_key_can_rotate_own_key_and_gets_not_found_for_other_tenants() {
    let (app, store, _platform_key) = test_app().await;
    let (_tenant_id, admin_key) = mint_admin_key(&store).await;

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api-keys")
                .header("content-type", "application/json")
                .header("x-api-key", &admin_key)
                .body(Body::from(
                    json!({ "label": "ci", "role": "standard" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_response.status(), StatusCode::OK);
    let created = body_json(create_response).await;
    let key_id = created["id"].as_str().unwrap().to_string();
    let old_raw_key = created["key"].as_str().unwrap().to_string();

    let rotate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api-keys/{key_id}/rotate"))
                .header("x-api-key", &admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rotate_response.status(), StatusCode::OK);
    let rotated = body_json(rotate_response).await;
    assert_eq!(rotated["id"], key_id, "rotation keeps the same key id");
    let new_raw_key = rotated["key"].as_str().unwrap().to_string();
    assert_ne!(new_raw_key, old_raw_key);

    // The old raw value must no longer authenticate.
    let old_key_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/spellcheck")
                .header("content-type", "application/json")
                .header("x-api-key", &old_raw_key)
                .body(Body::from(json!({ "text": "hello" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(old_key_response.status(), StatusCode::UNAUTHORIZED);

    // Rotating another tenant's key id must 404, not leak/succeed.
    let (tenant_b, _admin_b) = store
        .create_tenant("Tenant B".to_string(), None, None, None, None)
        .await
        .unwrap();
    let key_b = store
        .create_key(&tenant_b.id, "b-key".to_string(), Role::Standard, None)
        .await
        .unwrap();
    let cross_tenant_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api-keys/{}/rotate", key_b.record.id))
                .header("x-api-key", admin_key)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_tenant_response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn repeated_auth_failures_from_same_ip_get_rate_limited() {
    let (app, _store, _platform_key) = test_app().await;

    // test_app()'s RateLimiter defaults to 10 failures/60s -> 429.
    // Router::oneshot has no real peer address, so every request here shares
    // the same fallback IP (0.0.0.0) — exactly what makes them count against
    // one shared rate-limit bucket.
    let mut last_status = StatusCode::OK;
    for _ in 0..15 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/spellcheck")
                    .header("content-type", "application/json")
                    .header("x-api-key", "rsk_not-a-real-key")
                    .body(Body::from(json!({ "text": "hello" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = response.status();
        if last_status == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(
        last_status,
        StatusCode::TOO_MANY_REQUESTS,
        "10 failed attempts from the same IP should trigger the auth rate limiter"
    );
}

#[tokio::test]
async fn unknown_tenant_id_returns_not_found_on_every_platform_route() {
    let (app, _store, platform_key) = test_app().await;
    let bogus_id = "00000000-0000-0000-0000-000000000000";

    let cases: Vec<(&str, String)> = vec![
        ("GET", format!("/tenants/{bogus_id}")),
        ("PATCH", format!("/tenants/{bogus_id}")),
        ("POST", format!("/tenants/{bogus_id}/suspend")),
        ("POST", format!("/tenants/{bogus_id}/reactivate")),
    ];

    for (method, uri) in cases {
        let mut builder = Request::builder()
            .method(method)
            .uri(&uri)
            .header("x-api-key", &platform_key);
        let body = if method == "PATCH" {
            builder = builder.header("content-type", "application/json");
            Body::from(json!({ "name": "whatever" }).to_string())
        } else {
            Body::empty()
        };
        let response = app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} {uri} should 404 for an unknown tenant id"
        );
    }
}
