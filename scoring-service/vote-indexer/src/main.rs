//! Vote indexer entry point.
//!
//! Consumes vote events from the `curation.votes` Kafka topic and indexes them
//! into PostgreSQL.

use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

    info!("vote-indexer starting...");

    todo!("vote-indexer: Kafka consumer not yet implemented");
}
