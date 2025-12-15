//! OpenSearch provider implementation.
//!
//! This module provides the concrete implementation of `SearchIndexProvider`
//! using the OpenSearch Rust crate.

use async_trait::async_trait;
use opensearch::{
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
    DeleteParts, OpenSearch, UpdateParts,
};
use serde_json::json;
use tracing::{debug, error, info};
use url::Url;
use uuid::Uuid;

use crate::errors::SearchIndexError;
use crate::interfaces::SearchIndexProvider;
use crate::opensearch::index_config::IndexConfig;
use crate::types::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, UnsetEntityPropertiesRequest,
    UpdateEntityRequest,
};
use crate::utils;

/// OpenSearch provider implementation.
///
/// Provides full-text search capabilities using OpenSearch as the backend.
///
/// # Example
///
/// ```ignore
/// use search_indexer_repository::opensearch::IndexConfig;
/// use search_indexer_repository::types::UpdateEntityRequest;
/// let config = IndexConfig::new("entities", 0);
/// let provider = OpenSearchProvider::new("http://localhost:9200", config).await?;
///
/// let request = UpdateEntityRequest {
///     entity_id: Uuid::new_v4().to_string(),
///     space_id: Uuid::new_v4().to_string(),
///     name: Some("Test Entity".to_string()),
///     description: Some("Description".to_string()),
///     ..Default::default()
/// };
/// // This will create the document if it doesn't exist, or update it if it does
/// provider.update_document(&request).await?;
/// ```
pub struct OpenSearchProvider {
    client: OpenSearch,
    index_config: IndexConfig,
}

impl OpenSearchProvider {
    /// Create a new OpenSearch provider connected to the specified URL.
    ///
    /// # Arguments
    ///
    /// * `url` - The OpenSearch server URL (e.g., "http://localhost:9200")
    /// * `index_config` - The index configuration containing alias and version
    ///
    /// # Returns
    ///
    /// * `Ok(OpenSearchProvider)` - A new provider instance
    /// * `Err(SearchIndexError)` - If connection setup fails
    pub async fn new(url: &str, index_config: IndexConfig) -> Result<Self, SearchIndexError> {
        let parsed_url =
            Url::parse(url).map_err(|e| SearchIndexError::connection(e.to_string()))?;

        let conn_pool = SingleNodeConnectionPool::new(parsed_url);
        let transport = TransportBuilder::new(conn_pool)
            .disable_proxy()
            .build()
            .map_err(|e| SearchIndexError::connection(e.to_string()))?;

        let client = OpenSearch::new(transport);

        info!(
            url = %url,
            alias = %index_config.alias,
            version = index_config.version,
            "Created OpenSearch provider"
        );

        Ok(Self {
            client,
            index_config,
        })
    }

    /// Generate a document ID from entity and space IDs.
    ///
    /// Uses format: `{entity_id}_{space_id}` to ensure uniqueness.
    fn document_id(entity_id: &Uuid, space_id: &Uuid) -> String {
        format!("{}_{}", entity_id, space_id)
    }

    /// Validate and sanitize property keys.
    ///
    /// Property keys must contain only alphanumeric characters and underscores.
    ///
    /// # Arguments
    ///
    /// * `property_keys` - Vector of property keys to validate
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all property keys are valid
    /// * `Err(SearchIndexError)` - If any property key is invalid
    fn validate_property_keys(property_keys: &[String]) -> Result<(), SearchIndexError> {
        if property_keys.is_empty() {
            return Err(SearchIndexError::validation(
                "At least one property key must be provided".to_string(),
            ));
        }

        for key in property_keys {
            if key.is_empty() {
                return Err(SearchIndexError::validation(
                    "Property keys cannot be empty".to_string(),
                ));
            }

            if !key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return Err(SearchIndexError::validation(format!(
                    "Property key '{}' contains invalid characters. Only alphanumeric characters and underscores are allowed",
                    key
                )));
            }
        }

        Ok(())
    }

    /// Create a Painless script to safely remove multiple fields from a document.
    ///
    /// The script checks if each field exists before removing it to prevent errors.
    /// This function validates property keys before generating the script to ensure
    /// no invalid scripts can be created.
    ///
    /// # Arguments
    ///
    /// * `property_keys` - Vector of property keys to remove
    ///
    /// # Returns
    ///
    /// * `Ok(String)` - A Painless script source string that removes the specified fields
    /// * `Err(SearchIndexError)` - If property keys are invalid
    fn create_unset_properties_script(
        property_keys: &[String],
    ) -> Result<String, SearchIndexError> {
        // Validate property keys before generating script
        Self::validate_property_keys(property_keys)?;

        Ok(property_keys
            .iter()
            .map(|key| {
                // Escape the key for use in Painless script
                // Since we've validated the key contains only alphanumeric and underscore,
                // we don't need complex escaping, but we'll still quote it properly
                format!(
                    "if (ctx._source.containsKey(\"{}\")) {{ ctx._source.remove(\"{}\") }}",
                    key, key
                )
            })
            .collect::<Vec<_>>()
            .join("; "))
    }
}

#[async_trait]
impl SearchIndexProvider for OpenSearchProvider {
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
    async fn update_document(&self, request: &UpdateEntityRequest) -> Result<(), SearchIndexError> {
        // Validate UUIDs
        let (entity_id, space_id) =
            utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

        let doc_id = Self::document_id(&entity_id, &space_id);

        // Build update document with only provided fields
        let mut doc = serde_json::Map::new();
        if let Some(ref name) = request.name {
            doc.insert("name".to_string(), json!(name));
        }
        if let Some(ref description) = request.description {
            doc.insert("description".to_string(), json!(description));
        }
        if let Some(ref avatar) = request.avatar {
            doc.insert("avatar".to_string(), json!(avatar));
        }
        if let Some(ref cover) = request.cover {
            doc.insert("cover".to_string(), json!(cover));
        }
        if let Some(entity_global_score) = request.entity_global_score {
            doc.insert(
                "entity_global_score".to_string(),
                json!(entity_global_score),
            );
        }
        if let Some(space_score) = request.space_score {
            doc.insert("space_score".to_string(), json!(space_score));
        }
        if let Some(entity_space_score) = request.entity_space_score {
            doc.insert("entity_space_score".to_string(), json!(entity_space_score));
        }

        if doc.is_empty() {
            // No fields to update
            return Ok(());
        }

        // Use upsert to create document if it doesn't exist
        // API reference: https://docs.opensearch.org/latest/api-reference/document-apis/update-document/#using-the-upsert-operation
        let response = self
            .client
            .update(UpdateParts::IndexId(&self.index_config.alias, &doc_id))
            .body(json!({
                "doc": doc,
                "doc_as_upsert": true
            }))
            .send()
            .await
            .map_err(|e| SearchIndexError::update(e.to_string()))?;

        let status = response.status_code();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "Update request failed");
            return Err(SearchIndexError::update(format!(
                "Update failed with status {}: {}",
                status, error_body
            )));
        }

        debug!(doc_id = %doc_id, "Document updated/created");
        Ok(())
    }

    /// Delete a document from the search index.
    ///
    /// This function deletes a document identified by entity_id and space_id. If the
    /// document doesn't exist, the operation is considered successful (no error is returned).
    ///
    /// # Arguments
    ///
    /// * `request` - The delete request containing entity_id and space_id
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the document was deleted (or didn't exist)
    /// * `Err(SearchIndexError)` - If the deletion fails
    async fn delete_document(&self, request: &DeleteEntityRequest) -> Result<(), SearchIndexError> {
        let (entity_id, space_id) =
            utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

        let doc_id = Self::document_id(&entity_id, &space_id);

        let response = self
            .client
            .delete(DeleteParts::IndexId(&self.index_config.alias, &doc_id))
            .send()
            .await
            .map_err(|e| SearchIndexError::delete(e.to_string()))?;

        let status = response.status_code();

        // 404 is acceptable - document may not exist
        if !status.is_success() && status.as_u16() != 404 {
            let error_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "Delete request failed");
            return Err(SearchIndexError::delete(format!(
                "Delete failed with status {}: {}",
                status, error_body
            )));
        }

        debug!(doc_id = %doc_id, "Document deleted");
        Ok(())
    }

    /// Update multiple documents in bulk and return a summary of successful and failed operations.
    ///
    /// This function updates multiple documents by calling `update_document` for each request
    /// and collecting the results. Returns a summary indicating which updates succeeded and
    /// which failed, along with error details for failed operations.
    ///
    /// # Arguments
    ///
    /// * `requests` - Slice of update requests, each containing entity_id, space_id, and optional fields
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains total count, succeeded count, failed count,
    ///   and individual results for each request with success status and optional error
    async fn bulk_update_documents(
        &self,
        requests: &[UpdateEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for request in requests {
            match SearchIndexProvider::update_document(self, request).await {
                Ok(()) => {
                    succeeded += 1;
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                }
                Err(e) => {
                    failed += 1;
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: false,
                        error: Some(e.clone()),
                    });
                }
            }
        }

        Ok(BatchOperationSummary {
            total: requests.len(),
            succeeded,
            failed,
            results,
        })
    }

    /// Delete multiple documents in bulk and return a summary of successful and failed operations.
    ///
    /// This function deletes multiple documents by calling `delete_document` for each request
    /// and collecting the results. Returns a summary indicating which deletions succeeded and
    /// which failed. Note that documents not found are considered successful deletions.
    ///
    /// # Arguments
    ///
    /// * `requests` - Slice of delete requests, each containing entity_id and space_id
    ///
    /// # Returns
    ///
    /// * `Ok(BatchOperationSummary)` - Contains total count, succeeded count, failed count,
    ///   and individual results for each request with success status and optional error
    ///
    /// # Note
    ///
    /// If a document doesn't exist, the deletion is considered successful (no error is recorded).
    async fn bulk_delete_documents(
        &self,
        requests: &[DeleteEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for request in requests {
            match SearchIndexProvider::delete_document(self, request).await {
                Ok(()) => {
                    succeeded += 1;
                    results.push(BatchOperationResult {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        success: true,
                        error: None,
                    });
                }
                Err(e) => {
                    // Document not found is considered a successful delete
                    if matches!(e, SearchIndexError::DocumentNotFound(_)) {
                        succeeded += 1;
                        results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            success: true,
                            error: None,
                        });
                    } else {
                        failed += 1;
                        results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            success: false,
                            error: Some(e.clone()),
                        });
                    }
                }
            }
        }

        Ok(BatchOperationSummary {
            total: requests.len(),
            succeeded,
            failed,
            results,
        })
    }

    /// Unset (remove) specific properties from a document.
    ///
    /// This function removes the specified property keys from a document using a Painless script.
    /// If a property doesn't exist, it is safely ignored. The document must exist (this is not an upsert).
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
    ) -> Result<(), SearchIndexError> {
        // Validate UUIDs
        let (entity_id, space_id) =
            utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

        let doc_id = Self::document_id(&entity_id, &space_id);

        // Build Painless script to safely remove multiple fields
        // Validation and sanitization of property_keys happens
        //  inside create_unset_properties_script
        let script_source = Self::create_unset_properties_script(&request.property_keys)?;

        // Use update API with script to remove fields
        let response = self
            .client
            .update(UpdateParts::IndexId(&self.index_config.alias, &doc_id))
            .body(json!({
                "script": {
                    "source": script_source,
                    "lang": "painless"
                }
            }))
            .send()
            .await
            .map_err(|e| SearchIndexError::update(e.to_string()))?;

        let status = response.status_code();
        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %error_body, "Unset properties request failed");
            return Err(SearchIndexError::update(format!(
                "Unset properties failed with status {}: {}",
                status, error_body
            )));
        }

        debug!(
            doc_id = %doc_id,
            property_keys = ?request.property_keys,
            "Document properties unset"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_id() {
        let entity_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let space_id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

        let doc_id = OpenSearchProvider::document_id(&entity_id, &space_id);

        assert_eq!(
            doc_id,
            "550e8400-e29b-41d4-a716-446655440000_6ba7b810-9dad-11d1-80b4-00c04fd430c8"
        );
    }

    #[test]
    fn test_validate_property_keys_valid() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "entity_global_score".to_string(),
            "test123".to_string(),
            "a".to_string(),
            "A".to_string(),
            "a1".to_string(),
            "_private".to_string(),
        ];
        assert!(OpenSearchProvider::validate_property_keys(&keys).is_ok());
    }

    #[test]
    fn test_validate_property_keys_empty_vec() {
        let keys = vec![];
        let result = OpenSearchProvider::validate_property_keys(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_validate_property_keys_empty_string() {
        let keys = vec!["".to_string()];
        let result = OpenSearchProvider::validate_property_keys(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_validate_property_keys_invalid_characters() {
        let test_cases = vec![
            ("name-with-dash", "contains dash"),
            ("name.with.dot", "contains dot"),
            ("name with space", "contains space"),
            ("name@symbol", "contains @"),
            ("name#hash", "contains #"),
            ("name$dollar", "contains $"),
            ("name%percent", "contains %"),
            ("name&and", "contains &"),
            ("name*star", "contains *"),
            ("name+plus", "contains +"),
            ("name=equals", "contains ="),
            ("name[ bracket", "contains ["),
            ("name] bracket", "contains ]"),
            ("name{ brace", "contains {"),
            ("name} brace", "contains }"),
            ("name|pipe", "contains |"),
            ("name\\backslash", "contains backslash"),
            ("name/forward", "contains forward slash"),
            ("name?question", "contains ?"),
            ("name:colon", "contains :"),
            ("name;semicolon", "contains ;"),
            ("name\"quote", "contains quote"),
            ("name'apostrophe", "contains apostrophe"),
            ("name<less", "contains <"),
            ("name>greater", "contains >"),
            ("name,comma", "contains comma"),
        ];

        for (key, description) in test_cases {
            let keys = vec![key.to_string()];
            let result = OpenSearchProvider::validate_property_keys(&keys);
            assert!(
                result.is_err(),
                "Expected error for key '{}' ({})",
                key,
                description
            );
            assert!(
                matches!(result.unwrap_err(), SearchIndexError::ValidationError(_)),
                "Expected ValidationError for key '{}'",
                key
            );
        }
    }

    #[test]
    fn test_validate_property_keys_mixed_valid_invalid() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "invalid-key".to_string(), // Invalid
        ];
        let result = OpenSearchProvider::validate_property_keys(&keys);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_unset_properties_script_single_key() {
        let keys = vec!["name".to_string()];
        let script = OpenSearchProvider::create_unset_properties_script(&keys).unwrap();
        assert_eq!(
            script,
            "if (ctx._source.containsKey(\"name\")) { ctx._source.remove(\"name\") }"
        );
    }

    #[test]
    fn test_create_unset_properties_script_multiple_keys() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "avatar".to_string(),
        ];
        let script = OpenSearchProvider::create_unset_properties_script(&keys).unwrap();
        assert!(script.contains("name"));
        assert!(script.contains("description"));
        assert!(script.contains("avatar"));
        assert!(script.contains("containsKey"));
        assert!(script.contains("remove"));
        // Should have semicolons separating the statements
        assert_eq!(script.matches(';').count(), 2);
    }

    #[test]
    fn test_create_unset_properties_script_multiple_keys_exact_format() {
        let keys = vec![
            "name".to_string(),
            "description".to_string(),
            "avatar".to_string(),
            "cover".to_string(),
            "entity_global_score".to_string(),
        ];
        let script = OpenSearchProvider::create_unset_properties_script(&keys).unwrap();

        // Verify exact script format
        let expected_script = "if (ctx._source.containsKey(\"name\")) { ctx._source.remove(\"name\") }; if (ctx._source.containsKey(\"description\")) { ctx._source.remove(\"description\") }; if (ctx._source.containsKey(\"avatar\")) { ctx._source.remove(\"avatar\") }; if (ctx._source.containsKey(\"cover\")) { ctx._source.remove(\"cover\") }; if (ctx._source.containsKey(\"entity_global_score\")) { ctx._source.remove(\"entity_global_score\") }";
        assert_eq!(script, expected_script);
    }

    #[test]
    fn test_create_unset_properties_script_with_underscore() {
        let keys = vec!["entity_global_score".to_string()];
        let script = OpenSearchProvider::create_unset_properties_script(&keys).unwrap();
        assert_eq!(
            script,
            "if (ctx._source.containsKey(\"entity_global_score\")) { ctx._source.remove(\"entity_global_score\") }"
        );
    }

    #[test]
    fn test_create_unset_properties_script_empty() {
        let keys = vec![];
        let result = OpenSearchProvider::create_unset_properties_script(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }

    #[test]
    fn test_create_unset_properties_script_invalid_key() {
        let keys = vec!["invalid-key".to_string()];
        let result = OpenSearchProvider::create_unset_properties_script(&keys);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SearchIndexError::ValidationError(_)
        ));
    }
}
