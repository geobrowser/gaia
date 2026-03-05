//! HTTP server exposing health check and topology query endpoints.

pub mod health;
pub mod topology;

use std::sync::Arc;

use rdkafka::admin::AdminClient;
use rdkafka::client::DefaultClientContext;
use tokio::task::JoinHandle;

use crate::topology::CanonicalGraphState;
use hermes_instrumentation::{error, info};
use search_indexer_repository::SearchIndexProvider;

/// Shared application state for all HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    /// The search index provider for checking OpenSearch connectivity.
    pub(crate) provider: Arc<dyn SearchIndexProvider>,
    /// Kafka admin client for checking broker connectivity.
    pub(crate) kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    /// Canonical graph topology state for subspace queries.
    pub(crate) topology_state: CanonicalGraphState,
}

/// Start the HTTP server.
///
/// The server exposes:
/// - `GET /healthz` - Liveness probe
/// - `GET /readyz` - Readiness probe
/// - `GET /topology/subspaces/:space_id` - Subspace query for SPACE scope expansion
/// - `GET /topology/distance/:space_id` - Distance from root query
/// - `GET /topology/root` - Canonical graph root ID
pub fn start_http_server(
    provider: Arc<dyn SearchIndexProvider>,
    kafka_admin: Arc<AdminClient<DefaultClientContext>>,
    topology_state: CanonicalGraphState,
    port: u16,
) -> JoinHandle<()> {
    let state = AppState {
        provider,
        kafka_admin,
        topology_state,
    };

    let app = health::routes().merge(topology::routes()).with_state(state);

    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{}", port);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => listener,
            Err(e) => {
                error!(error = %e, addr = %addr, "Failed to bind HTTP server");
                return;
            }
        };

        info!(addr = %addr, "HTTP server listening");

        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "HTTP server error");
        }
    })
}
