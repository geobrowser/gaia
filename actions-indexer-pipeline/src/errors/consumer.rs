//! Error types for the consumer module of the Actions Indexer Pipeline.
//! Defines specific errors that can occur during the consumption of action events.
use thiserror::Error;

/// Represents errors that can occur within the action consumer.
///
/// This enum consolidates various error conditions specific to the consumption
/// process, such as placeholder errors for unimplemented functionality.
#[derive(Debug, Error, Clone)]
pub enum ConsumerError {
    #[error("Error reading package: {0}")]
    ReadingPackage(String),
    #[error("Error reading block range: {0}")]
    ReadingBlockRange(String),
    #[error("Error reading endpoint: {0}")]
    ReadingEndpoint(String),
    #[error("Error loading cursor: {0}")]
    LoadingCursor(String),
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Error decoding actions: {0}")]
    DecodingActions(String),
    #[error("Error processing block undo signal: {0}")] 
    ProcessingBlockUndoSignal(String),
    #[error("Error sending message through channel: {0}")]
    ChannelSend(String),
    #[error("Streaming error: {0}")]
    StreamingError(String),
    #[error("Error processing block scoped data: {0}")]
    ProcessingBlockScopedData(String),
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
}
