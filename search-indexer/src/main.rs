//! Search Indexer Main Entry Point
//!
//! This is the main binary for the Geo Knowledge Graph search indexer.
//! It consumes entity events from Kafka and indexes them into OpenSearch.

use dotenv::dotenv;
use search_indexer::{Dependencies, IndexingError};
use std::env;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Initialize tracing/logging.
fn init_tracing() -> Result<(), IndexingError> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("search_indexer=info,search_indexer_ingest=info"));

    let axiom_token = env::var("AXIOM_TOKEN").ok();

    if axiom_token.is_some() {
        // With Axiom token, use JSON format for structured logging
        tracing_subscriber::registry()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_target(true)
                    .with_thread_ids(true),
            )
            .init();

        info!(
            service_name = "search-indexer",
            service_version = env!("CARGO_PKG_VERSION"),
            "Tracing initialized with JSON format"
        );
    } else {
        // Without Axiom, use pretty console output
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(true).pretty())
            .init();

        info!(
            service_name = "search-indexer",
            service_version = env!("CARGO_PKG_VERSION"),
            "Tracing initialized with console output"
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize tracing
    init_tracing()?;

    info!("Starting Geo Search Indexer");

    // Initialize dependencies
    let deps = match Dependencies::new().await {
        Ok(deps) => {
            info!("Dependencies initialized successfully");
            deps
        }
        Err(e) => {
            error!(error = %e, "Failed to initialize dependencies");
            return Err(e);
        }
    };

    // Run the orchestrator
    match deps.orchestrator.run().await {
        Ok(()) => {
            info!("Search indexer completed successfully");
            Ok(())
        }
        Err(e) => {
            error!(error = %e, "Search indexer failed");
            Err(e.into())
        }
    }
}
