use thiserror::Error;

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

    /// Whether this error is worth retrying — i.e. the same message could
    /// succeed on a later attempt.
    ///
    /// The consumer commits offsets even on failure, deliberately, so a
    /// poison-pill message cannot stall the partition. That is right for a
    /// permanent error (an undecodable payload will never decode) but wrong
    /// for a transient one: a statement timeout or a dropped connection then
    /// skips the block for good and silently loses every event in it. Callers
    /// should retry while this returns true and only commit-and-skip once it
    /// returns false.
    pub fn is_transient(&self) -> bool {
        match self {
            IndexerError::Database(e) => match e {
                // No connection was established / the pool could not hand one
                // out — nothing was executed, so a retry is always safe.
                sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => true,
                sqlx::Error::Database(db) => matches!(
                    db.code().as_deref(),
                    // 57014 query_canceled — includes statement_timeout, the
                    // failure mode that dropped blocks 50294/50296/50298.
                    Some("57014")
                    // 40001 serialization_failure, 40P01 deadlock_detected —
                    // both resolve on retry by definition.
                    | Some("40001")
                    | Some("40P01")
                    // 53300 too_many_connections, 55P03 lock_not_available,
                    // 08006 connection_failure.
                    | Some("53300")
                    | Some("55P03")
                    | Some("08006")
                ),
                _ => false,
            },
            // A Kafka-side failure is an infrastructure condition, not bad data.
            IndexerError::Kafka(_) => true,
            // Bad or unsupported payloads never become valid by waiting.
            IndexerError::Handler(_) | IndexerError::Decode(_) | IndexerError::Config(_) => false,
        }
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
        assert!(!IndexerError::config("missing var").is_transient());
        assert!(!IndexerError::Handler(HandlerError::MissingPayload).is_transient());
    }

    #[test]
    fn infrastructure_errors_are_retried() {
        assert!(IndexerError::kafka("broker unavailable").is_transient());
        assert!(IndexerError::Database(sqlx::Error::PoolTimedOut).is_transient());
        assert!(IndexerError::Database(sqlx::Error::PoolClosed).is_transient());
    }

    #[test]
    fn row_not_found_is_not_transient() {
        // Guards the catch-all arm: a logical sqlx error must not be swept
        // into the retryable bucket just because it is an sqlx::Error.
        assert!(!IndexerError::Database(sqlx::Error::RowNotFound).is_transient());
    }
}
