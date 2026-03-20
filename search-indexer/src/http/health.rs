//! Kubernetes liveness and readiness probe handlers.

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
use hermes_instrumentation::error;
use std::time::Duration;

use super::AppState;

/// Liveness probe handler.
///
/// Returns 200 OK if the process is alive and able to serve requests.
/// This endpoint is used by Kubernetes to determine if the container should be restarted.
async fn liveness() -> impl IntoResponse {
    (StatusCode::OK, "alive")
}

/// Readiness probe handler.
///
/// Returns 200 OK if the service is ready to accept traffic.
/// Checks if both OpenSearch and Kafka connections are healthy.
async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    // Check OpenSearch connectivity
    if let Err(e) = state.provider.ensure_index_exists().await {
        error!(error = %e, "Readiness check failed: OpenSearch not accessible");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("not ready: opensearch error: {}", e),
        );
    }

    // Check Kafka broker connectivity by fetching metadata
    match tokio::task::spawn_blocking({
        let kafka_admin = state.kafka_admin.clone();
        move || {
            kafka_admin
                .inner()
                .fetch_metadata(None, Duration::from_secs(5))
        }
    })
    .await
    {
        Ok(Ok(_metadata)) => {
            // Both OpenSearch and Kafka are accessible
            (StatusCode::OK, "ready".to_string())
        }
        Ok(Err(e)) => {
            error!(error = %e, "Readiness check failed: Kafka broker not accessible");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("not ready: kafka error: {}", e),
            )
        }
        Err(e) => {
            error!(error = %e, "Readiness check failed: Kafka metadata fetch task failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("not ready: kafka task error: {}", e),
            )
        }
    }
}

/// Health check routes: `/healthz` and `/readyz`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
}
