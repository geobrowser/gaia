//! Error types for the consumer module of the Actions Indexer Pipeline.
//! Defines specific errors that can occur during the consumption of action events.
use thiserror::Error;

/// Errors related to configuration and initialization.
#[derive(Debug, Error, Clone)]
pub enum ConfigError {
    #[error("Error reading package: {0}")]
    ReadingPackage(String),
    #[error("Error reading block range: {0}")]
    ReadingBlockRange(String),
    #[error("Error reading endpoint: {0}")]
    ReadingEndpoint(String),
    #[error("Error loading cursor: {0}")]
    LoadingCursor(String),
}

/// Errors related to streaming and processing.
#[derive(Debug, Error, Clone)]
pub enum StreamError {
    #[error("Stream error: {0}")]
    Stream(String),
    #[error("Streaming error: {0}")]
    Streaming(String),
    #[error("Error processing block undo signal: {0}")]
    ProcessingBlockUndoSignal(String),
    #[error("Error processing block scoped data: {0}")]
    ProcessingBlockScopedData(String),
    #[error("Error sending message through channel: {0}")]
    ChannelSend(String),
    #[error("Error decoding actions: {0}")]
    DecodingActions(String),
}

/// Errors related to data conversion and validation.
#[derive(Debug, Error, Clone)]
pub enum ConversionError {
    #[error("Invalid address: {0}")]
    InvalidAddress(String),
    #[error("Invalid UUID: {0}")]
    InvalidUuid(String),
    #[error("Invalid transaction hash: {0}")]
    InvalidTxHash(String),
    #[error("Missing field: {0}")]
    MissingField(String),
    #[error("Invalid action type: {0}")]
    InvalidActionType(String),
    #[error("Invalid object type: {0}")]
    InvalidObjectType(String),
    #[error("Invalid vote direction: {0}")]
    InvalidVoteDirection(String),
    #[error("Invalid data field: {0}")]
    InvalidDataField(String),
}

/// Errors related to Kafka operations.
#[derive(Debug, Error, Clone)]
pub enum KafkaError {
    #[error("Kafka connection error: {0}")]
    Connection(String),
    #[error("Kafka subscription error: {0}")]
    Subscription(String),
    #[error("Kafka consume error: {0}")]
    Consume(String),
    #[error("Kafka commit error: {0}")]
    Commit(String),
}

/// Represents errors that can occur within the action consumer.
///
/// This enum consolidates various error conditions specific to the consumption
/// process by aggregating sub-error types for configuration, streaming,
/// conversion, and Kafka operations.
#[derive(Debug, Error, Clone)]
pub enum ConsumerError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Stream(#[from] StreamError),
    #[error(transparent)]
    Conversion(#[from] ConversionError),
    #[error(transparent)]
    Kafka(#[from] KafkaError),
}