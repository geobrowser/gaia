//! Health check HTTP server for Kubernetes liveness and readiness probes.
//!
//! This module provides a simple HTTP server that exposes health check endpoints
//! for Kubernetes to monitor the search-indexer service.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use rdkafka::admin::AdminClient;
use rdkafka::client::DefaultClientContext;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{error, info};

use search_indexer_repository::SearchIndexProvider;

/// Health check server state.
#[derive(Clone)]
pub struct HealthState {
    /// The search index provider for checking OpenSearch connectivity.
    provider: Arc<dyn SearchIndexProvider>,
    /// Kafka admin client for checking broker connectivity.
    kafka_admin: Arc<AdminClient<DefaultClientContext>>,
}

impl HealthState {
    /// Create a new health check state.
    pub fn new(
        provider: Arc<dyn SearchIndexProvider>,
        kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    ) -> Self {
        Self {
            provider,
            kafka_admin,
        }
    }
}

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
async fn readiness(State(state): State<HealthState>) -> impl IntoResponse {
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
        move || kafka_admin.inner().fetch_metadata(None, Duration::from_secs(5))
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

/// Start the health check HTTP server.
///
/// The server exposes two endpoints:
/// - `GET /healthz` - Liveness probe (always returns 200 if process is running)
/// - `GET /readyz` - Readiness probe (checks OpenSearch and Kafka connectivity)
///
/// # Arguments
///
/// * `provider` - The search index provider for checking OpenSearch connectivity
/// * `kafka_admin` - Kafka admin client for checking broker connectivity
/// * `port` - The port to bind the server to (default: 8080)
///
/// # Returns
///
/// A tokio task handle for the health check server.
pub fn start_health_server(
    provider: Arc<dyn SearchIndexProvider>,
    kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    port: u16,
) -> JoinHandle<()> {
    let state = HealthState::new(provider, kafka_admin);

    let app = Router::new()
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
        .with_state(state);

    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{}", port);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => listener,
            Err(e) => {
                error!(error = %e, addr = %addr, "Failed to bind health check server");
                return;
            }
        };

        info!(addr = %addr, "Health check server listening");

        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "Health check server error");
        }
    })
}
