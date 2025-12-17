//! Error types for the cursor repository.
//! Defines specific errors that can occur during database operations related to the cursor.
use thiserror::Error;

#[derive(Debug, Error)]
/// Represents errors that can occur within the cursor repository.
///
/// This enum consolidates various error conditions specific to database interactions,
/// such as SQLx errors during database operations.
pub enum CursorRepositoryError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}
