//! HTTP health check endpoints for Kubernetes probes.

use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use sqlx::PgPool;
use tokio::task::JoinHandle;

#[derive(Clone)]
struct HealthState {
    pool: PgPool,
}

/// Liveness probe — returns 200 if the process is running.
async fn liveness() -> impl IntoResponse {
    (StatusCode::OK, "alive")
}

/// Readiness probe — returns 200 if the database is reachable.
///
/// Times out after 5 seconds to prevent slow DB queries from blocking the probe.
async fn readiness(State(state): State<HealthState>) -> impl IntoResponse {
    match tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT 1").execute(&state.pool),
    )
    .await
    {
        Ok(Ok(_)) => (StatusCode::OK, "ready".to_string()),
        Ok(Err(e)) => (StatusCode::SERVICE_UNAVAILABLE, format!("not ready: {}", e)),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: db check timed out".to_string(),
        ),
    }
}

/// Spawn a health check HTTP server on the given port.
///
/// Returns a `JoinHandle` that can be used to await or abort the server.
pub fn start_health_server(pool: PgPool, port: u16) -> JoinHandle<()> {
    let state = HealthState { pool };
    let app = Router::new()
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
        .with_state(state);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
            .await
            .unwrap_or_else(|e| panic!("Failed to bind health server on port {}: {}", port, e));
        tracing::info!(port = port, "Health server listening");
        axum::serve(listener, app).await.ok();
    })
}
