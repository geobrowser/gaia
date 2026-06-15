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
}

impl From<prost::DecodeError> for IndexerError {
    fn from(e: prost::DecodeError) -> Self {
        Self::Decode(format!("prost: {e}"))
    }
}
