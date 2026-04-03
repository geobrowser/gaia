//! Error types for the notification-indexer.

use thiserror::Error;

/// Errors that can occur during event handling.
#[derive(Debug, Error)]
pub enum HandlerError {
    #[error("missing metadata in message")]
    MissingMetadata,

    #[error("uuid error: {0}")]
    Uuid(#[from] uuid::Error),

    #[error("unknown event type: {0}")]
    UnknownEventType(String),

    #[error("invalid vote option: {0}")]
    InvalidVoteOption(i32),

    #[error("invalid voting mode: {0}")]
    InvalidVotingMode(i32),

    #[error("grc-20 decode error: {0}")]
    Grc20Decode(String),
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
