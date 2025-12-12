//! Search index error types.
//!
//! This module defines the unified error type for all search index operations,
//! including both low-level backend errors and high-level application errors.

use thiserror::Error;

/// Unified errors from search index operations.
///
/// Used by the `SearchIndexProvider` trait and `SearchIndexService` for all search index
/// operations. Includes both low-level backend errors (connection, serialization, etc.)
/// and high-level application errors (validation, business logic, etc.).
#[derive(Debug, Clone, Error)]
pub enum SearchIndexError {
    /// Validation error (e.g., missing required fields, invalid UUIDs).
    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Failed to establish connection to the search index backend.
    #[error("Connection error: {0}")]
    ConnectionError(String),

    /// Failed to index a document.
    #[error("Index error: {0}")]
    IndexError(String),

    /// Bulk indexing operation had failures.
    #[error("Bulk index error: {0}")]
    BulkIndexError(String),

    /// Failed to update a document.
    #[error("Update error: {0}")]
    UpdateError(String),

    /// Failed to delete a document.
    #[error("Delete error: {0}")]
    DeleteError(String),

    /// Failed to create the search index.
    #[error("Index creation error: {0}")]
    IndexCreationError(String),

    /// Failed to parse response from search index backend.
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Failed to serialize data for the search index backend.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Document not found.
    #[error("Document not found: {0}")]
    DocumentNotFound(String),

    /// Batch size exceeds configured maximum.
    #[error("Batch size {provided} exceeds maximum {max}")]
    BatchSizeExceeded { provided: usize, max: usize },

    /// Unknown error.
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl SearchIndexError {
    /// Create a validation error.
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::ValidationError(msg.into())
    }

    /// Create a connection error.
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::ConnectionError(msg.into())
    }

    /// Create an index error.
    pub fn index(msg: impl Into<String>) -> Self {
        Self::IndexError(msg.into())
    }

    /// Create a bulk index error.
    pub fn bulk_index(msg: impl Into<String>) -> Self {
        Self::BulkIndexError(msg.into())
    }

    /// Create an update error.
    pub fn update(msg: impl Into<String>) -> Self {
        Self::UpdateError(msg.into())
    }

    /// Create a delete error.
    pub fn delete(msg: impl Into<String>) -> Self {
        Self::DeleteError(msg.into())
    }

    /// Create an index creation error.
    pub fn index_creation(msg: impl Into<String>) -> Self {
        Self::IndexCreationError(msg.into())
    }

    /// Create a parse error.
    pub fn parse(msg: impl Into<String>) -> Self {
        Self::ParseError(msg.into())
    }

    /// Create a serialization error.
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::SerializationError(msg.into())
    }

    /// Create a document not found error.
    pub fn document_not_found(entity_id: &str, space_id: &str) -> Self {
        Self::DocumentNotFound(format!("entity_id={}, space_id={}", entity_id, space_id))
    }

    /// Create a bulk operation error (alias for bulk_index for compatibility).
    pub fn bulk_operation(msg: impl Into<String>) -> Self {
        Self::BulkIndexError(msg.into())
    }

    /// Create a batch size exceeded error.
    pub fn batch_size_exceeded(provided: usize, max: usize) -> Self {
        Self::BatchSizeExceeded { provided, max }
    }

    /// Create an unknown error.
    pub fn unknown(msg: impl Into<String>) -> Self {
        Self::Unknown(msg.into())
    }
}
