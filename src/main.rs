//! Rust Spell Server - Production-ready Hunspell-compatible HTTP API
//!
//! This application exposes the `spellbook` spell-checking engine over a type-safe
//! HTTP API with full observability, validation, and graceful shutdown support.

use std::sync::Arc;

use rustspell_server::{
    auth, config,
    dictionary::DictionaryManager,
    engine::{Engine, EngineRegistry},
    handlers::{self, AppState},
    metrics,
    store::Store,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber
    let config = config::load()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::new(&config.log_level)
                .add_directive("rustspell_server=info".parse().unwrap()),
        )
        .init();

    // Initialize Prometheus metrics recorder
    let metrics_handle = metrics::install_recorder()?;

    // Download/load dictionary
    let dict_manager = DictionaryManager::new(&config);
    let (aff_path, dic_path) = dict_manager.ensure_dictionary(&config.language).await?;
    ::metrics::counter!("dictionary_refresh_total", "result" => "success").increment(1);
    let engine = Engine::load_from_paths(&aff_path, &dic_path)?;
    let engines = EngineRegistry::new(config.language.clone(), engine, dict_manager);

    // Open the key/tenant store; print the bootstrap platform key if this is
    // the first start (or the store was emptied of active platform keys).
    let (store, bootstrap_key) = Store::open(&config).await?;
    if let Some(created) = bootstrap_key {
        println!(
            "Bootstrap platform API key (save this now, it will not be shown again):\n  {}",
            created.raw_key
        );
        tracing::info!("bootstrap platform API key created");
    }

    let rate_limiter = auth::RateLimiter::new(
        config.auth_rate_limit_max,
        std::time::Duration::from_secs(config.auth_rate_limit_window_seconds),
        std::time::Duration::from_secs(config.auth_rate_limit_cooldown_seconds),
    );

    let app_state = Arc::new(AppState::new(
        Arc::new(engines),
        Arc::new(config.clone()),
        Arc::new(store),
        Arc::new(rate_limiter),
    ));

    // Build public API router
    let app = handlers::build_app(app_state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));
    let api_listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind API server to {addr}: {e}"))?;

    tracing::info!("API server listening on http://{addr}");

    // Spawn metrics server (failure does not take down the API)
    let metrics_port = config.metrics_port;
    let metrics_handle_clone = metrics_handle.clone();
    tokio::spawn(async move {
        if let Err(e) = metrics::serve(metrics_port, metrics_handle_clone).await {
            tracing::error!("metrics server error: {e}");
        }
    });

    // Run API server with graceful shutdown. Connect info is required for
    // per-IP auth-failure rate limiting (see `auth::require_active_key`).
    let shutdown = shutdown_signal();
    axum::serve(
        api_listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    .map_err(|e| anyhow::anyhow!("API server error: {e}"))?;
    tracing::info!("API server shut down gracefully");

    Ok(())
}

/// Handles graceful shutdown via SIGINT/SIGTERM on Unix and SIGINT on Windows.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await
    };

    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("SIGINT received, starting graceful shutdown"),
        _ = term => tracing::info!("SIGTERM received, starting graceful shutdown"),
    }
}
