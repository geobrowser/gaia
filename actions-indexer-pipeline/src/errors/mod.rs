//! Error types for the Actions Indexer Pipeline.
//! Consolidates and re-exports error types from various pipeline components
//! such as the processor, orchestrator, loader, and consumer.
mod consumer;
mod loader;
mod orchestrator;
mod processor;

pub use consumer::ConsumerError;
pub use loader::LoaderError;
pub use orchestrator::OrchestratorError;
pub use processor::ProcessorError;
