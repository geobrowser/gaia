//! Error types for the search indexer ingest.

use thiserror::Error;

/// Errors that can occur in the search indexer ingest.
#[derive(Error, Debug)]
pub enum IngestError {
    /// Error from the loader component.
    #[error("Loader error: {0}")]
    LoaderError(String),

    /// Kafka-related error.
    #[error("Kafka error: {0}")]
    KafkaError(String),

    /// Error parsing or decoding data.
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Channel communication error.
    #[error("Channel error: {0}")]
    ChannelError(String),

    /// Error from the orchestrator component.
    #[error("Orchestrator error: {0}")]
    OrchestratorError(String),
}

impl IngestError {
    /// Create a loader error.
    pub fn loader(msg: impl Into<String>) -> Self {
        Self::LoaderError(msg.into())
    }

    /// Create a Kafka error.
    pub fn kafka(msg: impl Into<String>) -> Self {
        Self::KafkaError(msg.into())
    }

    /// Create a parse error.
    pub fn parse(msg: impl Into<String>) -> Self {
        Self::ParseError(msg.into())
    }
}

impl From<rdkafka::error::KafkaError> for IngestError {
    fn from(err: rdkafka::error::KafkaError) -> Self {
        Self::KafkaError(err.to_string())
    }
}

/// Errors that can occur during indexer initialization or execution.
#[derive(Error, Debug)]
pub enum IndexingError {
    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Ingest error.
    #[error("Ingest error: {0}")]
    IngestError(#[from] IngestError),
}

impl IndexingError {
    /// Create a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::ConfigError(msg.into())
    }
}

impl From<hermes_instrumentation::Error> for IndexingError {
    fn from(err: hermes_instrumentation::Error) -> Self {
        Self::ConfigError(err.to_string())
    }
}
