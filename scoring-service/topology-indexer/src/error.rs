use thiserror::Error;

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
