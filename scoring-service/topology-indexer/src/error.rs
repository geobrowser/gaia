use thiserror::Error;

use crate::write_retry::{classify_sqlx, ErrorClass};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("conversion error: {0}")]
    Conversion(String),
}

#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("kafka error: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),
    #[error("decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("telemetry error: {0}")]
    Telemetry(#[from] hermes_instrumentation::Error),
    #[error("configuration error: {0}")]
    Config(String),
}

impl IndexerError {
    /// Whether a later attempt at the same diff could succeed. See
    /// `write_retry::classify_sqlx` for which database errors are permanent and
    /// why everything unclassified is transient.
    pub fn class(&self) -> ErrorClass {
        match self {
            IndexerError::Storage(StorageError::Database(e)) => classify_sqlx(e),
            // A distance that does not fit the column, or a payload that does not
            // decode, is the same on every attempt.
            IndexerError::Storage(StorageError::Conversion(_)) | IndexerError::Decode(_) => {
                ErrorClass::Permanent
            }
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
    fn conversion_errors_are_permanent_and_pool_errors_transient() {
        assert_eq!(
            IndexerError::Storage(StorageError::Conversion("too far".into())).class(),
            ErrorClass::Permanent
        );
        assert_eq!(
            IndexerError::Storage(StorageError::Database(sqlx::Error::PoolTimedOut)).class(),
            ErrorClass::Transient
        );
        assert_eq!(
            IndexerError::Config("x".into()).class(),
            ErrorClass::Transient
        );
    }
}
