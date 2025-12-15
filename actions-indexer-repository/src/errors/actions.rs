//! Error types for the actions repository.
//! Defines specific errors that can occur during database operations related to actions.
use thiserror::Error;

/// Represents errors that can occur within the actions repository.
///
/// This enum consolidates various error conditions specific to database interactions,
/// such as SQLx errors during database operations.
#[derive(Debug, Error)]
pub enum ActionsRepositoryError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(u64),

    #[error("Invalid vote type: {0}")]
    InvalidVoteType(i16),

    #[error("Invalid object type: {0}")]
    InvalidObjectType(i16),
}