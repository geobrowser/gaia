//! Error types for the Actions Indexer Pipeline.
//! Consolidates and re-exports error types from various pipeline components
//! such as the processor, orchestrator, loader, and consumer.
mod processor;
mod orchestrator;
mod loader;
mod consumer;

pub use processor::ProcessorError;
pub use orchestrator::OrchestratorError;
pub use loader::LoaderError;
pub use consumer::{ConfigError, ConsumerError, ConversionError, KafkaError, StreamError};