//! Rust Spell Server - Production-ready Hunspell-compatible HTTP API
//!
//! This application exposes the `spellbook` spell-checking engine over a type-safe
//! HTTP API with full observability, validation, and graceful shutdown support.

use std::sync::Arc;

use rustspell_server::{
    config,
    dictionary::DictionaryManager,
    engine::Engine,
    handlers::{self, AppState},
    metrics,
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
    let (aff_path, dic_path) = dict_manager.ensure_dictionary().await?;
    ::metrics::counter!("dictionary_refresh_total", "result" => "success").increment(1);
    let engine = Engine::load_from_paths(&aff_path, &dic_path)?;
    let app_state = Arc::new(AppState::new(Arc::new(engine), Arc::new(config.clone())));

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

    // Run API server with graceful shutdown
    let shutdown = shutdown_signal();
    axum::serve(api_listener, app)
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
