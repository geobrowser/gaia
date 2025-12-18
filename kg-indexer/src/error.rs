use thiserror::Error;

#[derive(Error, Debug)]
pub enum IndexerError {
    #[error("Kafka error: {0}")]
    Kafka(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Config error: {0}")]
    Config(String),
}

impl IndexerError {
    pub fn kafka(msg: impl Into<String>) -> Self {
        IndexerError::Kafka(msg.into())
    }

    pub fn parse(msg: impl Into<String>) -> Self {
        IndexerError::Parse(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        IndexerError::Config(msg.into())
    }
}
