//! # Search Indexer
//!
//! Search indexer for the Geo Knowledge Graph - consumes events from Kafka
//! and indexes them into OpenSearch.
//!
//! ## Architecture
//!
//! The indexer follows the Consumer-Processor-Loader pattern:
//!
//! 1. **Consumer**: Receives events from Kafka
//! 2. **Processor**: Transforms events into search documents
//! 3. **Loader**: Indexes documents into OpenSearch
//! 4. **Orchestrator**: Coordinates the ingest flow
//!
//! ## Modules
//!
//! - [`config`]: Configuration and dependency initialization
//! - [`consumer`]: Kafka consumer for entity events
//! - [`processor`]: Transforms events into documents
//! - [`loader`]: Indexes documents into OpenSearch
//! - [`orchestrator`]: Coordinates the ingest flow
//! - [`metrics`]: Metrics and telemetry for the indexer
//! - [`errors`]: Error types for the indexer

pub mod config;
pub mod consumer;
pub mod errors;
pub mod loader;
pub mod metrics;
pub mod orchestrator;
pub mod processor;

pub use config::Dependencies;
pub use errors::{IndexingError, IngestError};
