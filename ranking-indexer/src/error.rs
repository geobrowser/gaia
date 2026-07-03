//! Error types for the ranking-indexer.

#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("config error: {0}")]
    Config(String),

    #[error("kafka error: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl IndexerError {
    pub fn decode(msg: impl Into<String>) -> Self {
        Self::Decode(msg.into())
    }

    /// True when retrying the same input can never succeed, so the message
    /// must be logged + skipped rather than retried (which would crash-loop
    /// the partition forever, since the offset is never committed).
    ///
    /// Two cases are poison:
    /// - `Decode` — a malformed message; the bytes will never parse.
    /// - A database **integrity-constraint violation** (SQLSTATE class `23`:
    ///   unique, foreign-key, not-null, check). These are deterministic — the
    ///   same input fails identically on every redelivery, so retrying can
    ///   never converge. The recompute writes are idempotent (`ON CONFLICT`),
    ///   so we should not hit these in practice; this is the safety net that
    ///   stops a future non-idempotent write from stalling the consumer group.
    ///
    /// Everything else — transient database errors (connection drops,
    /// deadlocks, serialization failures), Kafka errors — stays transient:
    /// retried, never skipped, since redelivery can converge.
    pub fn is_poison(&self) -> bool {
        match self {
            Self::Decode(_) => true,
            Self::Database(e) => e
                .as_database_error()
                .and_then(|db| db.code())
                .is_some_and(|code| code.starts_with("23")),
            _ => false,
        }
    }
}

impl From<prost::DecodeError> for IndexerError {
    fn from(e: prost::DecodeError) -> Self {
        Self::Decode(format!("prost: {e}"))
    }
}
