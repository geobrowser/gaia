use thiserror::Error;

/// Errors that can occur when processing messages in handlers
#[derive(Error, Debug)]
pub enum HandlerError {
    #[error("Invalid space ID: {0}")]
    InvalidSpaceId(String),

    #[error("Invalid UUID bytes: {0}")]
    InvalidUuidBytes(#[from] uuid::Error),

    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Missing payload in message")]
    MissingPayload,

    #[error("Unknown membership role: {0}")]
    UnknownRole(i32),
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
}
