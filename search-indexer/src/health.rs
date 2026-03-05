//! Health check HTTP server for Kubernetes liveness and readiness probes.
//!
//! This module provides a simple HTTP server that exposes health check endpoints
//! for Kubernetes to monitor the search-indexer service.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use hermes_instrumentation::{error, info};
use rdkafka::admin::AdminClient;
use rdkafka::client::DefaultClientContext;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::topology::CanonicalGraphState;
use search_indexer_repository::SearchIndexProvider;

/// Health check server state.
#[derive(Clone)]
pub struct HealthState {
    /// The search index provider for checking OpenSearch connectivity.
    provider: Arc<dyn SearchIndexProvider>,
    /// Kafka admin client for checking broker connectivity.
    kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    /// Canonical graph topology state for subspace queries.
    topology_state: CanonicalGraphState,
}

impl HealthState {
    /// Create a new health check state.
    pub fn new(
        provider: Arc<dyn SearchIndexProvider>,
        kafka_admin: Arc<AdminClient<DefaultClientContext>>,
        topology_state: CanonicalGraphState,
    ) -> Self {
        Self {
            provider,
            kafka_admin,
            topology_state,
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

/// Parse a space_id from a path parameter.
/// Accepts both dashed UUID (36 chars) and 32-char hex.
fn parse_space_id(space_id_str: &str) -> Option<[u8; 16]> {
    // Uuid::parse_str accepts both dashed (36-char) and simple hex (32-char) formats.
    uuid::Uuid::parse_str(space_id_str)
        .ok()
        .map(|uuid| *uuid.as_bytes())
}

/// GET /topology/subspaces/:space_id
///
/// Returns all subspaces (descendants + self) for a canonical space.
async fn get_subspaces(
    State(state): State<HealthState>,
    Path(space_id_str): Path<String>,
) -> impl IntoResponse {
    let space_bytes = match parse_space_id(&space_id_str) {
        Some(bytes) => bytes,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid space_id format" })),
            );
        }
    };

    match state.topology_state.get_subspaces(&space_bytes) {
        Some(subspaces) => {
            let count = subspaces.len();
            let subspace_strs: Vec<String> = subspaces.iter().map(|u| u.to_string()).collect();
            let is_root = state
                .topology_state
                .root_id()
                .map(|r| *r.as_bytes() == space_bytes)
                .unwrap_or(false);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "space_id": space_id_str,
                    "subspaces": subspace_strs,
                    "count": count,
                    "is_root": is_root,
                })),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "space not found in canonical graph",
                "space_id": space_id_str,
            })),
        ),
    }
}

/// GET /topology/distance/:space_id
///
/// Returns the distance from root for a canonical space.
async fn get_distance(
    State(state): State<HealthState>,
    Path(space_id_str): Path<String>,
) -> impl IntoResponse {
    let space_bytes = match parse_space_id(&space_id_str) {
        Some(bytes) => bytes,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid space_id format" })),
            );
        }
    };

    match state.topology_state.get_distance(&space_bytes) {
        Some(distance) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "space_id": space_id_str,
                "distance": distance,
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "space not found in canonical graph",
                "space_id": space_id_str,
            })),
        ),
    }
}

/// GET /topology/root
///
/// Returns the canonical graph root space ID.
async fn get_root(State(state): State<HealthState>) -> impl IntoResponse {
    match state.topology_state.root_id() {
        Some(root_uuid) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "root_id": root_uuid.to_string(),
            })),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no root set yet",
            })),
        ),
    }
}

/// Start the health check HTTP server.
///
/// The server exposes:
/// - `GET /healthz` - Liveness probe
/// - `GET /readyz` - Readiness probe
/// - `GET /topology/subspaces/:space_id` - Subspace query for SPACE scope expansion
/// - `GET /topology/distance/:space_id` - Distance from root query
/// - `GET /topology/root` - Canonical graph root ID
pub fn start_health_server(
    provider: Arc<dyn SearchIndexProvider>,
    kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    topology_state: CanonicalGraphState,
    port: u16,
) -> JoinHandle<()> {
    let state = HealthState::new(provider, kafka_admin, topology_state);

    let app = Router::new()
        .route("/healthz", get(liveness))
        .route("/readyz", get(readiness))
        .route("/topology/subspaces/{space_id}", get(get_subspaces))
        .route("/topology/distance/{space_id}", get(get_distance))
        .route("/topology/root", get(get_root))
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
