use thiserror::Error;

use crate::write_retry::{classify_sqlx, ErrorClass};

/// Errors that can occur when processing messages in handlers
#[derive(Error, Debug)]
pub enum HandlerError {
    #[error("Invalid space ID: {0}")]
    InvalidSpaceId(String),

    #[error("Invalid UUID bytes: {0}")]
    InvalidUuidBytes(#[from] uuid::Error),

    #[error("Missing payload in message")]
    MissingPayload,

    #[error("Unknown membership role: {0}")]
    UnknownRole(i32),

    #[error("Decode error: {0}")]
    DecodeError(String),
}

/// Top-level errors for the indexer
#[derive(Error, Debug)]
pub enum IndexerError {
    #[error("Kafka error: {0}")]
    Kafka(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Handler error: {0}")]
    Handler(#[from] HandlerError),

    #[error("Decode error: {0}")]
    Decode(String),

    #[error("Config error: {0}")]
    Config(String),
}

impl IndexerError {
    pub fn kafka(msg: impl Into<String>) -> Self {
        IndexerError::Kafka(msg.into())
    }

    pub fn decode(msg: impl Into<String>) -> Self {
        IndexerError::Decode(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        IndexerError::Config(msg.into())
    }

    /// Whether a later attempt at the same block could succeed.
    ///
    /// Transient failures are retried and, if they persist, halt the consumer
    /// without committing, so the block is re-read after the restart (GEO-2884).
    /// Permanent failures are skipped and counted, because retrying a message
    /// that can never be written would block the partition forever.
    ///
    /// Only errors that are provably about the message itself are permanent: a
    /// payload that does not decode or that a handler rejects, and a database
    /// error in SQLSTATE class 22 (data exception) or 23 (integrity constraint).
    /// Everything else, including errors nobody has thought about, is transient;
    /// see `write_retry::classify_sqlx` for why that is the safe default.
    pub fn class(&self) -> ErrorClass {
        match self {
            IndexerError::Database(e) => classify_sqlx(e),
            // Bad or unsupported payloads never become valid by waiting.
            IndexerError::Handler(_) | IndexerError::Decode(_) => ErrorClass::Permanent,
            // A Kafka-side failure is an infrastructure condition, not bad data.
            // A config error mid-batch is a deploy problem: skipping would drop
            // every block until it was fixed.
            IndexerError::Kafka(_) | IndexerError::Config(_) => ErrorClass::Transient,
        }
    }

    pub fn is_transient(&self) -> bool {
        self.class() == ErrorClass::Transient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permanent_errors_are_not_retried() {
        // A payload that cannot be decoded will not decode later either;
        // retrying these is what stalls a partition on a poison pill.
        assert!(!IndexerError::decode("bad payload").is_transient());
        assert!(!IndexerError::Handler(HandlerError::MissingPayload).is_transient());
    }

    #[test]
    fn infrastructure_errors_are_retried() {
        assert!(IndexerError::kafka("broker unavailable").is_transient());
        assert!(IndexerError::Database(sqlx::Error::PoolTimedOut).is_transient());
        assert!(IndexerError::Database(sqlx::Error::PoolClosed).is_transient());
    }

    #[test]
    fn unclassified_errors_halt_rather_than_skip() {
        // The catch-all is transient on purpose: skipping on an error nobody
        // classified is how a whole block goes missing without anyone knowing.
        assert!(IndexerError::Database(sqlx::Error::RowNotFound).is_transient());
        assert!(IndexerError::config("missing var").is_transient());
    }
}
