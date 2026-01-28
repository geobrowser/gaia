//! Error types for the vote-indexer.

use thiserror::Error;

/// Errors that can occur during vote handling.
#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("missing payload in vote message")]
    MissingPayload,

    #[error("invalid object type: {0:?}")]
    InvalidObjectType(Vec<u8>),

    #[error("invalid vote direction: {0}")]
    InvalidVoteDirection(i32),

    #[error("uuid error: {0}")]
    Uuid(#[from] uuid::Error),
}

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Top-level indexer error.
#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("handler error: {0}")]
    Handler(#[from] HandlerError),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("kafka error: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),

    #[error("decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("telemetry error: {0}")]
    Telemetry(#[from] hermes_instrumentation::Error),

    #[error("configuration error: {0}")]
    Config(String),
}
