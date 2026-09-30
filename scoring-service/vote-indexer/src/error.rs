//! Error types for the vote-indexer.

use thiserror::Error;

use crate::write_retry::{classify_sqlx, ErrorClass};

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

impl IndexerError {
    /// Whether a later attempt at the same write could succeed. See
    /// `write_retry::classify_sqlx` for which database errors count as permanent
    /// and why everything unclassified is transient.
    pub fn class(&self) -> ErrorClass {
        match self {
            IndexerError::Storage(StorageError::Database(e)) | IndexerError::Database(e) => {
                classify_sqlx(e)
            }
            // A message that does not decode or does not make a vote never will.
            IndexerError::Handler(_) | IndexerError::Decode(_) => ErrorClass::Permanent,
            // Infrastructure and deploy problems: skipping would lose every vote
            // until they were fixed.
            IndexerError::Kafka(_) | IndexerError::Telemetry(_) | IndexerError::Config(_) => {
                ErrorClass::Transient
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_messages_are_permanent_and_infrastructure_is_transient() {
        assert_eq!(
            IndexerError::Handler(HandlerError::MissingPayload).class(),
            ErrorClass::Permanent
        );
        assert_eq!(
            IndexerError::Handler(HandlerError::InvalidVoteDirection(9)).class(),
            ErrorClass::Permanent
        );
        assert_eq!(
            IndexerError::Storage(StorageError::Database(sqlx::Error::PoolTimedOut)).class(),
            ErrorClass::Transient
        );
        assert_eq!(
            IndexerError::Database(sqlx::Error::PoolClosed).class(),
            ErrorClass::Transient
        );
        assert_eq!(
            IndexerError::Config("x".into()).class(),
            ErrorClass::Transient
        );
    }
}
