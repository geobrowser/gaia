//! OpenSearch provider implementation.
//!
//! This module provides the concrete implementation of `SearchIndexProvider`
//! using the OpenSearch Rust crate.

use async_trait::async_trait;
use opensearch::{
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
    indices::{IndicesCreateParts, IndicesExistsParts, IndicesGetAliasParts},
    BulkOperation, DeleteParts, OpenSearch, UpdateParts,
};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument, warn};
use url::Url;
use uuid::Uuid;

use crate::errors::SearchIndexError;
use crate::interfaces::SearchIndexProvider;
use crate::opensearch::bulk::{
    execute_bulk, BulkAction, BulkOperationMeta, BulkScript, BulkScriptBody, BulkUpdateBody,
};
use crate::opensearch::index_config::{get_index_settings, get_versioned_index_name, IndexConfig};
use crate::opensearch::unset_document_properties::create_unset_properties_script;
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

    /// Generate document ID as `{entity_id}_{space_id}`.
    fn document_id(entity_id: &Uuid, space_id: &Uuid) -> String {
        format!("{}_{}", entity_id, space_id)
    }

    /// Build a document map from an UpdateEntityRequest with only the provided fields.
    fn build_update_doc(request: &UpdateEntityRequest) -> serde_json::Map<String, Value> {
        let mut doc = serde_json::Map::new();
        // Always include entity_id and space_id
        doc.insert("entity_id".to_string(), json!(request.entity_id));
        doc.insert("space_id".to_string(), json!(request.space_id));
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
        doc
    }
}

#[async_trait]
impl SearchIndexProvider for OpenSearchProvider {
    /// Ensure the versioned index and alias exist, creating them if necessary.
    ///
    /// This method performs the following steps:
    /// 1. Check if the versioned index (e.g., "entities_v0") exists; create it if not
    /// 2. Check if the alias (e.g., "entities") exists and points to the correct index
    /// 3. Create or update the alias to point to the versioned index
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the index and alias are ready for use
    /// * `Err(SearchIndexError)` - If index or alias operations fail
    #[instrument(skip(self))]
    async fn ensure_index_exists(&self) -> Result<(), SearchIndexError> {
        // Get the versioned index name (e.g., "entities_v0")
        let versioned_index_name = get_versioned_index_name(Some(self.index_config.version));

        // Step 1: Ensure the versioned index exists
        let index_exists_response = self
            .client
            .indices()
            .exists(IndicesExistsParts::Index(&[&versioned_index_name]))
            .send()
            .await
            .map_err(|e| SearchIndexError::connection(e.to_string()))?;

        if !index_exists_response.status_code().is_success() {
            info!(index = %versioned_index_name, "Creating versioned index");

            let settings = get_index_settings(Some(self.index_config.version));

            let create_response = self
                .client
                .indices()
                .create(IndicesCreateParts::Index(&versioned_index_name))
                .body(settings)
                .send()
                .await
                .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

            let status = create_response.status_code();
            if !status.is_success() {
                let error_body = create_response.text().await.unwrap_or_default();
                error!(status = %status, body = %error_body, "Index creation failed");
                return Err(SearchIndexError::index_creation(format!(
                    "Index creation failed with status {}: {}",
                    status, error_body
                )));
            }

            info!(index = %versioned_index_name, "Versioned index created successfully");
        } else {
            debug!(index = %versioned_index_name, "Versioned index already exists");
        }

        // Step 2: Check if alias exists and create/update it
        // Use get_alias to check if alias exists and what it points to
        let get_alias_response = self
            .client
            .indices()
            .get_alias(IndicesGetAliasParts::Name(&[&self.index_config.alias]))
            .send()
            .await;

        let alias_exists = get_alias_response.is_ok()
            && get_alias_response
                .as_ref()
                .map(|resp| resp.status_code().is_success())
                .unwrap_or(false);

        if !alias_exists {
            // Alias doesn't exist, create it
            info!(alias = %self.index_config.alias, index = %versioned_index_name, "Creating alias");

            let actions = json!({
                "actions": [
                    {
                        "add": {
                            "index": versioned_index_name,
                            "alias": self.index_config.alias
                        }
                    }
                ]
            });

            let update_response = self
                .client
                .indices()
                .update_aliases()
                .body(actions)
                .send()
                .await
                .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

            let status = update_response.status_code();
            if !status.is_success() {
                let error_body = update_response.text().await.unwrap_or_default();
                error!(status = %status, body = %error_body, "Alias creation failed");
                return Err(SearchIndexError::index_creation(format!(
                    "Alias creation failed with status {}: {}",
                    status, error_body
                )));
            }

            info!(alias = %self.index_config.alias, index = %versioned_index_name, "Alias created successfully");
        } else {
            // Alias exists, check if it points to the correct index
            let alias_body: Value = get_alias_response
                .map_err(|e| {
                    SearchIndexError::connection(format!("Failed to get alias response: {}", e))
                })?
                .json()
                .await
                .map_err(|e| SearchIndexError::parse(e.to_string()))?;

            // Check if alias points to the versioned index
            let points_to_correct_index = alias_body
                .as_object()
                .and_then(|obj| obj.get(&versioned_index_name))
                .is_some();

            if !points_to_correct_index {
                // Update alias to point to the correct index
                // First, remove alias from all indices it currently points to
                warn!(
                    alias = %self.index_config.alias,
                    expected_index = %versioned_index_name,
                    "Alias points to different index, updating"
                );

                let mut actions = Vec::new();
                if let Some(indices) = alias_body.as_object() {
                    for index_name in indices.keys() {
                        actions.push(json!({
                            "remove": {
                                "index": index_name,
                                "alias": self.index_config.alias
                            }
                        }));
                    }
                }
                // Then add alias to the correct index
                actions.push(json!({
                    "add": {
                        "index": versioned_index_name,
                        "alias": self.index_config.alias
                    }
                }));

                let update_response = self
                    .client
                    .indices()
                    .update_aliases()
                    .body(json!({ "actions": actions }))
                    .send()
                    .await
                    .map_err(|e| SearchIndexError::index_creation(e.to_string()))?;

                let status = update_response.status_code();
                if !status.is_success() {
                    let error_body = update_response.text().await.unwrap_or_default();
                    error!(status = %status, body = %error_body, "Alias update failed");
                    return Err(SearchIndexError::index_creation(format!(
                        "Alias update failed with status {}: {}",
                        status, error_body
                    )));
                }

                info!(alias = %self.index_config.alias, index = %versioned_index_name, "Alias updated successfully");
            } else {
                debug!(alias = %self.index_config.alias, index = %versioned_index_name, "Alias already points to correct index");
            }
        }

        Ok(())
    }

    /// Update specific fields of a document, creating it if it doesn't exist (upsert).
    ///
    /// This function performs an upsert operation: if the document exists, only fields that are
    /// `Some` in the request will be updated; if the document doesn't exist, it will be created
    /// with the provided fields. Fields that are `None` in the request will be left unchanged
    /// (for existing documents) or omitted (for new documents).
    async fn update_document(&self, request: &UpdateEntityRequest) -> Result<(), SearchIndexError> {
        // Validate UUIDs
        let (entity_id, space_id) =
            utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

        let doc_id = Self::document_id(&entity_id, &space_id);
        let doc = Self::build_update_doc(request);

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

    /// Update multiple documents in bulk using the OpenSearch bulk API.
    #[instrument(skip(self, requests), fields(count = requests.len()))]
    async fn bulk_update_documents(
        &self,
        requests: &[UpdateEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        // Validate all requests and build operations
        let mut operations: Vec<BulkOperation<BulkUpdateBody>> = Vec::with_capacity(requests.len());
        let mut metas: Vec<BulkOperationMeta> = Vec::with_capacity(requests.len());
        let mut skipped_empty: Vec<BatchOperationResult> = Vec::new();

        for request in requests {
            // Fail fast on validation error
            let (entity_id, space_id) =
                utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

            let doc = Self::build_update_doc(request);

            // Skip if no fields to update
            if doc.is_empty() {
                skipped_empty.push(BatchOperationResult {
                    entity_id: request.entity_id.clone(),
                    space_id: request.space_id.clone(),
                    success: true,
                    error: None,
                });
                continue;
            }

            let doc_id = Self::document_id(&entity_id, &space_id);
            let body = BulkUpdateBody {
                doc: Value::Object(doc),
                doc_as_upsert: true,
            };
            operations.push(BulkOperation::update(doc_id, body).into());
            metas.push(BulkOperationMeta {
                entity_id: request.entity_id.clone(),
                space_id: request.space_id.clone(),
            });
        }

        // If no operations to execute, return early with skipped results
        if operations.is_empty() {
            return Ok(BatchOperationSummary {
                total: requests.len(),
                succeeded: skipped_empty.len(),
                failed: 0,
                results: skipped_empty,
            });
        }

        let mut summary = execute_bulk(
            &self.client,
            &self.index_config.alias,
            operations,
            &metas,
            BulkAction::Update,
        )
        .await?;

        // Add skipped (empty) operations as successes
        summary.succeeded += skipped_empty.len();
        summary.total = requests.len();
        summary.results.extend(skipped_empty);

        Ok(summary)
    }

    /// Delete multiple documents in bulk using the OpenSearch bulk API.
    ///
    /// Note that documents not found are considered successful deletions.
    #[instrument(skip(self, requests), fields(count = requests.len()))]
    async fn bulk_delete_documents(
        &self,
        requests: &[DeleteEntityRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        if requests.is_empty() {
            return Ok(BatchOperationSummary::empty());
        }

        // Validate all requests and build operations
        let mut operations: Vec<BulkOperation<()>> = Vec::with_capacity(requests.len());
        let mut metas: Vec<BulkOperationMeta> = Vec::with_capacity(requests.len());

        for request in requests {
            // Fail fast on validation error
            let (entity_id, space_id) =
                utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

            let doc_id = Self::document_id(&entity_id, &space_id);
            operations.push(BulkOperation::delete(doc_id).into());
            metas.push(BulkOperationMeta {
                entity_id: request.entity_id.clone(),
                space_id: request.space_id.clone(),
            });
        }

        execute_bulk(
            &self.client,
            &self.index_config.alias,
            operations,
            &metas,
            BulkAction::Delete,
        )
        .await
    }

    /// Unset (remove) specific properties from a document.
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
        let script_source = create_unset_properties_script(&request.property_keys)?;

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

    /// Unset properties from multiple documents in bulk using the OpenSearch bulk API.
    #[instrument(skip(self, requests), fields(count = requests.len()))]
    async fn bulk_unset_properties(
        &self,
        requests: &[UnsetEntityPropertiesRequest],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        // Validate all requests and build operations
        let mut operations: Vec<BulkOperation<BulkScriptBody>> = Vec::with_capacity(requests.len());
        let mut metas: Vec<BulkOperationMeta> = Vec::with_capacity(requests.len());

        for request in requests {
            // Fail fast on validation errors
            let (entity_id, space_id) =
                utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;
            let script_source = create_unset_properties_script(&request.property_keys)?;

            let doc_id = Self::document_id(&entity_id, &space_id);
            let body = BulkScriptBody {
                script: BulkScript {
                    source: script_source,
                    lang: "painless",
                },
            };
            operations.push(BulkOperation::update(doc_id, body).into());
            metas.push(BulkOperationMeta {
                entity_id: request.entity_id.clone(),
                space_id: request.space_id.clone(),
            });
        }

        execute_bulk(
            &self.client,
            &self.index_config.alias,
            operations,
            &metas,
            BulkAction::Update,
        )
        .await
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
}
