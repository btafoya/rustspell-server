//! Prometheus metrics recorder and mini HTTP server.

use std::net::SocketAddr;
use std::time::Instant;

use axum::{routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;

/// Install the Prometheus recorder and return the render handle.
pub fn install_recorder() -> anyhow::Result<PrometheusHandle> {
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    let handle = builder.install_recorder()?;
    Ok(handle)
}

/// Build a router that serves `/metrics` from the given Prometheus handle.
pub fn metrics_router(handle: PrometheusHandle) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    )
}

/// Spawn the metrics server on the configured port.
pub async fn serve(port: u16, handle: PrometheusHandle) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("failed to bind metrics server to {addr}: {e}"))?;

    tracing::info!("Metrics server listening on http://{addr}/metrics");

    let app = metrics_router(handle);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Convenience macro helpers for recording request metrics.
pub fn record_request_start() -> Instant {
    Instant::now()
}

pub fn record_request_end(method: &str, path: &str, status: u16, start: Instant) {
    let duration = start.elapsed().as_secs_f64();
    metrics::counter!("http_requests_total", "method" => method.to_string(), "path" => path.to_string(), "status" => status.to_string())
        .increment(1);
    metrics::histogram!("http_request_duration_seconds", "method" => method.to_string(), "path" => path.to_string())
        .record(duration);
}
