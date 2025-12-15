//! Search index service implementation.
//!
//! This module provides the main service for interacting with the search index.
//! Application code uses this to update and delete documents.
//!
//! # Note on Document Creation
//!
//! There is no separate `create` function because all grc-20 events are edits (updates).
//! The `update` function performs an upsert operation: it will create the document if
//! it doesn't exist, or update it if it does exist.

use crate::config::SearchIndexServiceConfig;
use crate::errors::SearchIndexError;
use crate::interfaces::SearchIndexProvider;
use crate::types::{
    BatchOperationSummary, DeleteEntityRequest, UnsetEntityPropertiesRequest, UpdateEntityRequest,
};
use uuid::Uuid;

/// The main service for interacting with the search index.
///
/// This is the high-level API that application code should use. It provides input
/// validation, request conversion, and delegates to a `SearchIndexProvider` for
/// actual backend operations. All operations return `SearchIndexError` for consistent
/// error handling.
///
/// # Note on Document Creation
///
/// There is no separate `create` function because all grc-20 events are edits (updates).
/// The `update` function performs an upsert operation: it will create the document if
/// it doesn't exist, or update it if it does exist.
///
/// # Example
///
/// ```no_run
/// use search_indexer_repository::{SearchIndexService, SearchIndexProvider};
/// use search_indexer_repository::opensearch::{OpenSearchProvider, IndexConfig};
/// use search_indexer_repository::UpdateEntityRequest;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = IndexConfig::new("entities", 0);
/// let provider = Box::new(OpenSearchProvider::new("http://localhost:9200", config).await?);
/// let service = SearchIndexService::new(provider);
///
/// let request = UpdateEntityRequest {
///     entity_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
///     space_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_string(),
///     name: Some("My Entity".to_string()),
///     description: None,
///     avatar: None,
///     cover: None,
///     entity_global_score: None,
///     space_score: None,
///     entity_space_score: None,
/// };
///
/// // This will create the document if it doesn't exist, or update it if it does
/// service.update(request).await?;
/// # Ok(())
/// # }
/// ```
pub struct SearchIndexService {
    provider: Box<dyn SearchIndexProvider>,
    config: SearchIndexServiceConfig,
}

impl SearchIndexService {
    /// Create a new SearchIndexService with default configuration.
    ///
    /// The default configuration includes a batch size limit of 1000 documents.
    ///
    /// # Arguments
    ///
    /// * `provider` - A boxed implementation of `SearchIndexProvider` (e.g., `OpenSearchProvider`)
    ///
    /// # Returns
    ///
    /// A new `SearchIndexService` instance with default configuration.
    pub fn new(provider: Box<dyn SearchIndexProvider>) -> Self {
        Self {
            provider,
            config: SearchIndexServiceConfig::default(),
        }
    }

    /// Create a new SearchIndexService with custom configuration.
    ///
    /// Use this when you need to customize batch size limits or other configuration options.
    ///
    /// # Arguments
    ///
    /// * `provider` - A boxed implementation of `SearchIndexProvider` (e.g., `OpenSearchProvider`)
    /// * `config` - Custom configuration for the service
    ///
    /// # Returns
    ///
    /// A new `SearchIndexService` instance with the specified configuration.
    pub fn with_config(
        provider: Box<dyn SearchIndexProvider>,
        config: SearchIndexServiceConfig,
    ) -> Self {
        Self { provider, config }
    }

    /// Check if batch size exceeds the configured limit.
    fn validate_batch_size(&self, size: usize) -> Result<(), SearchIndexError> {
        if let Some(max) = self.config.max_batch_size {
            if size > max {
                return Err(SearchIndexError::batch_size_exceeded(size, max));
            }
        }
        Ok(())
    }

    /// Validate that a string is a valid UUID format.
    fn validate_uuid(field_name: &str, value: &str) -> Result<(), SearchIndexError> {
        if value.is_empty() {
            return Err(SearchIndexError::validation(format!(
                "{} is required",
                field_name
            )));
        }
        Uuid::parse_str(value).map_err(|e| {
            SearchIndexError::validation(format!("{} must be a valid UUID: {}", field_name, e))
        })?;
        Ok(())
    }

    /// Update one or more properties of a document, creating it if it doesn't exist (upsert).
    ///
    /// This function performs an upsert operation: if the document exists, only fields that are
    /// `Some` in the request will be updated; if the document doesn't exist, it will be created
    /// with the provided fields. Fields that are `None` will be left unchanged (for existing
    /// documents) or omitted (for new documents). The entity_id and space_id are required and
    /// must be valid UUIDs.
    ///
    /// # Arguments
    ///
    /// * `request` - UpdateEntityRequest containing entity_id, space_id, and optional fields
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the document was updated or created successfully
    /// * `Err(SearchIndexError::ValidationError)` - If UUIDs are invalid
    /// * `Err(SearchIndexError)` - If the operation fails
    ///
    /// # Note
    ///
    /// There is no separate `create` function because all grc-20 events are edits (updates).
    /// This function handles both creating new documents and updating existing ones.
    pub async fn update(&self, request: UpdateEntityRequest) -> Result<(), SearchIndexError> {
        // Validate required fields and UUID format
        Self::validate_uuid("entity_id", &request.entity_id)?;
        Self::validate_uuid("space_id", &request.space_id)?;

        // Build partial document update with only provided fields
        // Send update request to provider
        self.provider.update_document(&request).await
    }

    /// Delete an entity document from the search index.
    ///
    /// This function deletes a document identified by entity_id and space_id. If the
    /// document doesn't exist, the operation is considered successful.
    ///
    /// # Arguments
    ///
    /// * `request` - DeleteEntityRequest containing entity_id and space_id
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the document was deleted (or didn't exist)
    /// * `Err(SearchIndexError::ValidationError)` - If UUIDs are invalid
    /// * `Err(SearchIndexError)` - If the deletion fails
    pub async fn delete(&self, request: DeleteEntityRequest) -> Result<(), SearchIndexError> {
        // Validate required fields and UUID format
        Self::validate_uuid("entity_id", &request.entity_id)?;
        Self::validate_uuid("space_id", &request.space_id)?;

        self.provider.delete_document(&request).await
    }

    /// Unset (remove) specific properties from an entity document.
    ///
    /// This function removes the specified property keys from a document. If a property
    /// doesn't exist, it is safely ignored. The document must exist (this is not an upsert).
    /// Common property keys include: "name", "description", "avatar", "cover", "entity_global_score",
    /// "space_score", "entity_space_score".
    ///
    /// # Arguments
    ///
    /// * `request` - UnsetEntityPropertiesRequest containing entity_id, space_id, and property keys to remove
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the properties were removed successfully
    /// * `Err(SearchIndexError::ValidationError)` - If UUIDs are invalid or no property keys provided
    /// * `Err(SearchIndexError)` - If the operation fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// use search_indexer_repository::{SearchIndexService, SearchIndexProvider};
    /// use search_indexer_repository::types::UnsetEntityPropertiesRequest;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let provider = Box::new(search_indexer_repository::opensearch::OpenSearchProvider::new("http://localhost:9200", search_indexer_repository::opensearch::IndexConfig::new("entities", 0)).await?);
    /// # let service = SearchIndexService::new(provider);
    /// let request = UnsetEntityPropertiesRequest {
    ///     entity_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
    ///     space_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_string(),
    ///     property_keys: vec!["description".to_string(), "avatar".to_string()],
    /// };
    ///
    /// service.unset_properties(request).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn unset_properties(
        &self,
        request: UnsetEntityPropertiesRequest,
    ) -> Result<(), SearchIndexError> {
        // Validate required fields and UUID format
        Self::validate_uuid("entity_id", &request.entity_id)?;
        Self::validate_uuid("space_id", &request.space_id)?;

        // Validate that at least one property key is provided
        if request.property_keys.is_empty() {
            return Err(SearchIndexError::validation(
                "At least one property key must be provided".to_string(),
            ));
        }

        self.provider.unset_document_properties(&request).await
    }

    /// Update multiple entity documents in bulk and return a summary of successful and failed operations.
    ///
    /// This function performs upsert operations on multiple documents: if a document exists, only
    /// fields that are `Some` in the request will be updated; if a document doesn't exist, it will
    /// be created with the provided fields. Returns a summary indicating which operations succeeded
    /// and which failed.
    ///
    /// # Arguments
    ///
    /// * `requests` - Vector of UpdateEntityRequest, each containing entity_id, space_id, and optional fields
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains total count, succeeded count, failed count,
    ///   and individual results for each request with success status and optional error
    /// * `Err(SearchIndexError::BatchSizeExceeded)` - If the batch size exceeds the configured maximum
    /// * `Err(SearchIndexError::ValidationError)` - If any request has invalid UUIDs
    /// * `Err(SearchIndexError)` - If the bulk operation fails entirely
    ///
    /// # Note
    ///
    /// The batch size is limited by the configured `max_batch_size` (default: 1000). Individual
    /// operation failures are reported in the summary rather than causing the entire operation to fail.
    /// There is no separate `batch_create` function because all grc-20 events are edits (updates).
    pub async fn batch_update(
        &self,
        requests: Vec<UpdateEntityRequest>,
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        if requests.is_empty() {
            return Ok(BatchOperationSummary {
                total: 0,
                succeeded: 0,
                failed: 0,
                results: vec![],
            });
        }

        self.validate_batch_size(requests.len())?;

        // Validate all requests (UUID format and required fields)
        for request in &requests {
            Self::validate_uuid("entity_id", &request.entity_id)?;
            Self::validate_uuid("space_id", &request.space_id)?;
        }

        self.provider.bulk_update_documents(&requests).await
    }

    /// Delete multiple entity documents in bulk and return a summary of successful and failed operations.
    ///
    /// This function deletes multiple documents by processing each delete request. Returns a
    /// summary indicating which deletions succeeded and which failed. Documents that don't
    /// exist are considered successful deletions.
    ///
    /// # Arguments
    ///
    /// * `requests` - Vector of DeleteEntityRequest, each containing entity_id and space_id
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains total count, succeeded count, failed count,
    ///   and individual results for each request with success status and optional error
    /// * `Err(SearchIndexError::BatchSizeExceeded)` - If the batch size exceeds the configured maximum
    /// * `Err(SearchIndexError::ValidationError)` - If any request has invalid UUIDs
    /// * `Err(SearchIndexError)` - If the bulk operation fails entirely
    ///
    /// # Note
    ///
    /// The batch size is limited by the configured `max_batch_size` (default: 1000). Individual
    /// deletion failures are reported in the summary rather than causing the entire operation to fail.
    pub async fn batch_delete(
        &self,
        requests: Vec<DeleteEntityRequest>,
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        if requests.is_empty() {
            return Ok(BatchOperationSummary {
                total: 0,
                succeeded: 0,
                failed: 0,
                results: vec![],
            });
        }

        self.validate_batch_size(requests.len())?;

        // Validate all requests (UUID format and required fields)
        for request in &requests {
            Self::validate_uuid("entity_id", &request.entity_id)?;
            Self::validate_uuid("space_id", &request.space_id)?;
        }

        self.provider.bulk_delete_documents(&requests).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BatchOperationResult;
    use async_trait::async_trait;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    /// Mock provider for testing
    struct MockProvider {
        update_requests: Arc<Mutex<Vec<UpdateEntityRequest>>>,
        delete_requests: Arc<Mutex<Vec<DeleteEntityRequest>>>,
        should_fail: bool,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                update_requests: Arc::new(Mutex::new(Vec::new())),
                delete_requests: Arc::new(Mutex::new(Vec::new())),
                should_fail: false,
            }
        }
    }

    #[async_trait]
    impl SearchIndexProvider for MockProvider {
        async fn update_document(
            &self,
            request: &UpdateEntityRequest,
        ) -> Result<(), SearchIndexError> {
            if self.should_fail {
                return Err(SearchIndexError::index("Mock failure"));
            }
            self.update_requests.lock().await.push(request.clone());
            Ok(())
        }

        async fn delete_document(
            &self,
            request: &DeleteEntityRequest,
        ) -> Result<(), SearchIndexError> {
            if self.should_fail {
                return Err(SearchIndexError::index("Mock failure"));
            }
            self.delete_requests.lock().await.push(request.clone());
            Ok(())
        }

        async fn bulk_update_documents(
            &self,
            requests: &[UpdateEntityRequest],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            if self.should_fail {
                return Err(SearchIndexError::bulk_operation("Mock failure"));
            }

            let mut results = Vec::new();
            let mut succeeded = 0;
            let failed = 0;

            for req in requests {
                let result = BatchOperationResult {
                    entity_id: req.entity_id.clone(),
                    space_id: req.space_id.clone(),
                    success: true,
                    error: None,
                };
                results.push(result);
                succeeded += 1;
                self.update_requests.lock().await.push(req.clone());
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded,
                failed,
                results,
            })
        }

        async fn bulk_delete_documents(
            &self,
            requests: &[DeleteEntityRequest],
        ) -> Result<BatchOperationSummary, SearchIndexError> {
            if self.should_fail {
                return Err(SearchIndexError::bulk_operation("Mock failure"));
            }

            let mut results = Vec::new();
            let mut succeeded = 0;
            let failed = 0;

            for req in requests {
                let result = BatchOperationResult {
                    entity_id: req.entity_id.clone(),
                    space_id: req.space_id.clone(),
                    success: true,
                    error: None,
                };
                results.push(result);
                succeeded += 1;
                self.delete_requests.lock().await.push(req.clone());
            }

            Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded,
                failed,
                results,
            })
        }

        async fn unset_document_properties(
            &self,
            _request: &UnsetEntityPropertiesRequest,
        ) -> Result<(), SearchIndexError> {
            if self.should_fail {
                return Err(SearchIndexError::index("Mock failure"));
            }
            // Mock implementation - just succeed without tracking
            Ok(())
        }
    }

    fn create_test_update_request(entity_id: &str, space_id: &str) -> UpdateEntityRequest {
        UpdateEntityRequest {
            entity_id: entity_id.to_string(),
            space_id: space_id.to_string(),
            name: Some("Updated name".to_string()),
            description: None,
            avatar: None,
            cover: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
        }
    }

    fn create_test_delete_request(entity_id: &str, space_id: &str) -> DeleteEntityRequest {
        DeleteEntityRequest {
            entity_id: entity_id.to_string(),
            space_id: space_id.to_string(),
        }
    }

    #[tokio::test]
    async fn test_batch_update_empty() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let result = service.batch_update(vec![]).await.unwrap();

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn test_batch_update_single() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let entity_id = Uuid::new_v4().to_string();
        let space_id = Uuid::new_v4().to_string();
        let requests = vec![create_test_update_request(&entity_id, &space_id)];

        let result = service.batch_update(requests).await.unwrap();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].success);
    }

    #[tokio::test]
    async fn test_batch_update_multiple() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let requests = vec![
            create_test_update_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
            create_test_update_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
            create_test_update_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
        ];

        let result = service.batch_update(requests).await.unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.results.len(), 3);
    }

    #[tokio::test]
    async fn test_batch_delete_empty() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let result = service.batch_delete(vec![]).await.unwrap();

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert!(result.results.is_empty());
    }

    #[tokio::test]
    async fn test_batch_delete_single() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let entity_id = Uuid::new_v4().to_string();
        let space_id = Uuid::new_v4().to_string();
        let requests = vec![create_test_delete_request(&entity_id, &space_id)];

        let result = service.batch_delete(requests).await.unwrap();

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].success);
    }

    #[tokio::test]
    async fn test_batch_delete_multiple() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        let requests = vec![
            create_test_delete_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
            create_test_delete_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
            create_test_delete_request(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string()),
        ];

        let result = service.batch_delete(requests).await.unwrap();

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.results.len(), 3);
    }

    #[tokio::test]
    async fn test_update_validation() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        // Test empty entity_id
        let request = UpdateEntityRequest {
            entity_id: "".to_string(),
            space_id: Uuid::new_v4().to_string(),
            name: None,
            description: None,
            avatar: None,
            cover: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
        };
        assert!(service.update(request).await.is_err());

        // Test empty space_id
        let request = UpdateEntityRequest {
            entity_id: Uuid::new_v4().to_string(),
            space_id: "".to_string(),
            name: None,
            description: None,
            avatar: None,
            cover: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
        };
        assert!(service.update(request).await.is_err());
    }

    #[tokio::test]
    async fn test_delete_validation() {
        let provider = MockProvider::new();
        let service = SearchIndexService::new(Box::new(provider));

        // Test empty entity_id
        let request = DeleteEntityRequest {
            entity_id: "".to_string(),
            space_id: Uuid::new_v4().to_string(),
        };
        assert!(service.delete(request).await.is_err());

        // Test empty space_id
        let request = DeleteEntityRequest {
            entity_id: Uuid::new_v4().to_string(),
            space_id: "".to_string(),
        };
        assert!(service.delete(request).await.is_err());
    }

    #[tokio::test]
    async fn test_batch_size_unlimited() {
        let provider = MockProvider::new();
        let config = SearchIndexServiceConfig::unlimited();
        let service = SearchIndexService::with_config(Box::new(provider), config);

        // Should allow any batch size
        let requests: Vec<UpdateEntityRequest> = (0..10000)
            .map(|i| UpdateEntityRequest {
                entity_id: Uuid::new_v4().to_string(),
                space_id: Uuid::new_v4().to_string(),
                name: Some(format!("Entity {}", i)),
                description: None,
                avatar: None,
                cover: None,
                entity_global_score: None,
                space_score: None,
                entity_space_score: None,
            })
            .collect();

        // This should not fail due to batch size (though it might fail for other reasons)
        let result = service.batch_update(requests).await;
        // If it fails, it should not be BatchSizeExceeded
        if let Err(SearchIndexError::BatchSizeExceeded { .. }) = result {
            panic!("Batch size should not be limited with unlimited config");
        }
    }
}
