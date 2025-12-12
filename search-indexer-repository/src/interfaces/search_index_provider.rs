//! Search index provider trait definition.
//!
//! This module defines the abstract interface for search index operations,
//! allowing for different backend implementations (OpenSearch, Elasticsearch, etc.).

use async_trait::async_trait;

use crate::errors::SearchIndexError;
use crate::types::{
    BatchOperationSummary, DeleteEntityRequest, UnsetEntityPropertiesRequest, UpdateEntityRequest,
};

/// Abstracts the underlying search index implementation (OpenSearch, Elasticsearch, etc.).
///
/// This trait defines the interface for all search index backend implementations. Implementations
/// are injected into `SearchIndexService` to enable dependency injection and easy testing with
/// mock implementations.
///
/// All methods return `Result<T, SearchIndexError>` for consistent error handling across
/// different backend implementations.
///
/// # Note on Document Creation
///
/// There is no separate `create_document` function because all grc-20 events are edits (updates).
/// The `update_document` function performs an upsert operation: it will create the document if
/// it doesn't exist, or update it if it does exist.
#[async_trait]
pub trait SearchIndexProvider: Send + Sync {
    /// Update specific fields of a document, creating it if it doesn't exist (upsert).
    ///
    /// This function performs an upsert operation: if the document exists, only fields that are
    /// `Some` in the request will be updated; if the document doesn't exist, it will be created
    /// with the provided fields. Fields that are `None` in the request will be left unchanged
    /// (for existing documents) or omitted (for new documents).
    ///
    /// # Arguments
    ///
    /// * `request` - The update request containing entity_id, space_id, and optional fields
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the document was updated or created successfully
    /// * `Err(SearchIndexError)` - If the operation fails
    async fn update_document(&self, request: &UpdateEntityRequest) -> Result<(), SearchIndexError>;

    /// Delete a document from the search index.
    ///
    /// If the document doesn't exist, the operation is considered successful.
    ///
    /// # Arguments
    ///
    /// * `request` - The delete request containing entity_id and space_id
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the document was deleted (or didn't exist)
    /// * `Err(SearchIndexError)` - If the deletion fails
    async fn delete_document(&self, request: &DeleteEntityRequest) -> Result<(), SearchIndexError>;

    /// Update multiple documents in bulk and return a summary of successful and failed operations.
    ///
    /// Processes each update request individually and collects results. Returns a summary
    /// indicating which updates succeeded and which failed.
    ///
    /// # Arguments
    ///
    /// * `requests` - Slice of update requests
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains aggregate statistics and individual results
    /// * `Err(SearchIndexError)` - If the bulk operation fails entirely
    async fn bulk_update_documents(
        &self,
        requests: &[UpdateEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError>;

    /// Delete multiple documents in bulk and return a summary of successful and failed operations.
    ///
    /// Processes each delete request individually and collects results. Documents that don't
    /// exist are considered successful deletions.
    ///
    /// # Arguments
    ///
    /// * `requests` - Slice of delete requests
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains aggregate statistics and individual results
    /// * `Err(SearchIndexError)` - If the bulk operation fails entirely
    async fn bulk_delete_documents(
        &self,
        requests: &[DeleteEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError>;

    /// Unset (remove) specific properties from a document.
    ///
    /// This function removes the specified property keys from a document. If a property
    /// doesn't exist, it is safely ignored. The document must exist (this is not an upsert).
    ///
    /// # Arguments
    ///
    /// * `request` - The unset request containing entity_id, space_id, and property keys to remove
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the properties were removed successfully
    /// * `Err(SearchIndexError)` - If the operation fails
    async fn unset_document_properties(
        &self,
        request: &UnsetEntityPropertiesRequest,
    ) -> Result<(), SearchIndexError>;
}
