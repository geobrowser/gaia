//! OpenSearch provider implementation.
//!
//! This module provides the concrete implementation of `SearchIndexProvider`
//! using the OpenSearch crate.

use std::collections::HashMap;

use async_trait::async_trait;
use opensearch::{
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
    params::Conflicts,
    BulkOperation, DeleteParts, OpenSearch, SearchParts, UpdateByQueryParts, UpdateParts,
};
use serde_json::{json, Value};
use tracing::{debug, error, info, instrument, warn};
use url::Url;
use uuid::Uuid;

use crate::errors::SearchIndexError;
use crate::interfaces::SearchIndexProvider;
use crate::opensearch::bulk::{execute_bulk, BulkAction, BulkOperationMeta};
use crate::opensearch::index_config::IndexConfig;
use crate::opensearch::index_management;
use crate::opensearch::scripts::{
    ADD_RELATION_SCRIPT, REMOVE_RELATION_SCRIPT, UPDATE_WITH_TOMBSTONE_CHECK_SCRIPT,
};

use crate::opensearch::unset_document_properties::create_unset_properties_script;
use crate::types::{
    BatchOperationResult, BatchOperationSummary, DeleteEntityRequest, EntityOperation,
    UnsetEntityPropertiesRequest, UpdateEntityRequest,
};
use crate::utils;

/// Macro to flush pending bulk operations and refresh before update_by_query.
/// This ensures ordering is preserved when mixing bulk and update_by_query operations.
/// When there are pending ops, they are flushed with `Refresh::True`. When the pending
/// queue is empty, an explicit index refresh is performed so that writes from earlier
/// batches (which may have been flushed without refresh) are visible to the subsequent
/// update_by_query's search phase.
/// A macro is used instead of a function because this code mutates multiple local variables
/// and uses `await?`, which would require passing many `&mut` references to a function.
macro_rules! flush_pending_bulk {
    ($self:expr, $bulk_ops:expr, $metas:expr, $total_succeeded:expr, $total_failed:expr, $all_results:expr) => {
        if !$bulk_ops.is_empty() {
            let batch_ops = std::mem::take(&mut $bulk_ops);
            let batch_metas = std::mem::take(&mut $metas);
            let summary = execute_bulk(
                &$self.client,
                &$self.index_config.alias,
                batch_ops,
                &batch_metas,
                BulkAction::Update,
                true, // refresh so followingupdate_by_query sees latest data
            )
            .await?;
            $total_succeeded += summary.succeeded;
            $total_failed += summary.failed;
            $all_results.extend(summary.results);
        } else {
            // No pending ops, but prior batches may have written without
            // refresh. Force a refresh so update_by_query sees all data.
            debug!("No pending bulk ops; issuing explicit index refresh before update_by_query");
            $self
                .client
                .indices()
                .refresh(opensearch::indices::IndicesRefreshParts::Index(&[&$self
                    .index_config
                    .alias]))
                .send()
                .await
                .map_err(|e| SearchIndexError::update(e.to_string()))?;
        }
    };
}

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
        if let Some(ref image_url) = request.image_url {
            doc.insert("image_url".to_string(), json!(image_url));
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
        if let Some(deleted) = request.deleted {
            doc.insert("deleted".to_string(), json!(deleted));
        }
        if let Some(ref space_topic_entity_id) = request.space_topic_entity_id {
            doc.insert(
                "space_topic_entity_id".to_string(),
                json!(space_topic_entity_id),
            );
        }
        doc
    }

    /// Fetch all space_id → space_topic_entity_id mappings from the index.
    ///
    /// Uses a composite aggregation to paginate through all spaces that have
    /// a `space_topic_entity_id` set, returning a map of space_id to topic_entity_id.
    /// This is used to warm the in-memory cache at startup.
    ///
    /// Each page fetches up to 10,000 unique space_ids, so for n unique spaces
    /// with a topic, the loop runs ⌈n / 10,000⌉ iterations (+ 1 final empty page).
    pub async fn get_space_topic_mappings(&self) -> Result<HashMap<Uuid, Uuid>, SearchIndexError> {
        let mut mappings = HashMap::new();
        let mut after_key: Option<Value> = None;

        loop {
            let mut composite_source = json!({
                "size": 10000,
                "sources": [
                    {
                        "space": {
                            "terms": {
                                "field": "space_id"
                            }
                        }
                    }
                ]
            });

            if let Some(ref after) = after_key {
                composite_source["after"] = after.clone();
            }

            let body = json!({
                "size": 0,
                "query": {
                    "exists": {
                        "field": "space_topic_entity_id"
                    }
                },
                "aggs": {
                    "spaces": {
                        "composite": composite_source,
                        "aggs": {
                            "topic": {
                                "terms": {
                                    "field": "space_topic_entity_id",
                                    "size": 1
                                }
                            }
                        }
                    }
                }
            });

            let response = self
                .client
                .search(SearchParts::Index(&[&self.index_config.alias]))
                .body(body)
                .send()
                .await
                .map_err(|e| SearchIndexError::connection(e.to_string()))?;

            let status = response.status_code();
            if !status.is_success() {
                let error_body = response.text().await.unwrap_or_default();
                return Err(SearchIndexError::connection(format!(
                    "Space topic mappings query failed with status {}: {}",
                    status, error_body
                )));
            }

            let response_body: Value = response
                .json()
                .await
                .map_err(|e| SearchIndexError::connection(e.to_string()))?;

            let buckets = match response_body["aggregations"]["spaces"]["buckets"].as_array() {
                Some(b) if !b.is_empty() => b,
                _ => break,
            };

            for bucket in buckets {
                let space_id_str = bucket["key"]["space"].as_str().unwrap_or_default();
                let topic_buckets = bucket["topic"]["buckets"].as_array();

                if let Some(topic_buckets) = topic_buckets {
                    if let Some(first_topic) = topic_buckets.first() {
                        let topic_id_str = first_topic["key"].as_str().unwrap_or_default();

                        match (Uuid::parse_str(space_id_str), Uuid::parse_str(topic_id_str)) {
                            (Ok(space_id), Ok(topic_id)) => {
                                mappings.insert(space_id, topic_id);
                            }
                            _ => {
                                warn!(
                                    space_id = %space_id_str,
                                    topic_id = %topic_id_str,
                                    "Skipping invalid UUID in space topic mapping"
                                );
                            }
                        }
                    }
                }
            }

            // Check for after_key for pagination
            after_key = response_body["aggregations"]["spaces"]["after_key"]
                .clone()
                .into();
            if after_key.as_ref().is_none_or(|v| v.is_null()) {
                break;
            }
        }

        Ok(mappings)
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
        index_management::ensure_index_exists(&self.client, &self.index_config).await
    }

    /// Update specific fields of a document, creating it if it doesn't exist (upsert).
    ///
    /// This function performs an upsert operation: if the document exists, only fields that are
    /// `Some` in the request will be updated; if the document doesn't exist, it will be created
    /// with the provided fields. Fields that are `None` in the request will be left unchanged
    /// (for existing documents) or omitted (for new documents).
    ///
    /// Also supports atomic relation addition via `add_relation` field.
    async fn update_document(&self, request: &UpdateEntityRequest) -> Result<(), SearchIndexError> {
        // Validate UUIDs
        let (entity_id, space_id) =
            utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;

        let doc_id = Self::document_id(&entity_id, &space_id);

        // Handle add_relation - use script for atomic addition to relations array
        if let Some(ref relation_data) = request.add_relation {
            let relation_id = uuid::Uuid::parse_str(&relation_data.relation_id).map_err(|_| {
                SearchIndexError::validation(format!(
                    "Invalid relation_id: {}",
                    relation_data.relation_id
                ))
            })?;
            let to_entity_id =
                uuid::Uuid::parse_str(&relation_data.to_entity_id).map_err(|_| {
                    SearchIndexError::validation(format!(
                        "Invalid to_entity_id: {}",
                        relation_data.to_entity_id
                    ))
                })?;

            let params = json!({
                "relation_id": relation_id.to_string(),
                "relation_type": relation_data.relation_type.clone(),
                "to_entity_id": to_entity_id.to_string()
            });

            let upsert_relation = json!({
                "relation_id": relation_id.to_string(),
                "relation_type": relation_data.relation_type.clone(),
                "to_entity_id": to_entity_id.to_string()
            });

            let response = self
                .client
                .update(UpdateParts::IndexId(&self.index_config.alias, &doc_id))
                .body(json!({
                    "script": {
                        "source": ADD_RELATION_SCRIPT,
                        "lang": "painless",
                        "params": params
                    },
                    "upsert": {
                        "entity_id": entity_id.to_string(),
                        "space_id": space_id.to_string(),
                        "relations": [upsert_relation]
                    }
                }))
                .send()
                .await
                .map_err(|e| SearchIndexError::update(e.to_string()))?;

            let status = response.status_code();
            if !status.is_success() {
                let error_body = response.text().await.unwrap_or_default();
                error!(status = %status, body = %error_body, "Add relation request failed");
                return Err(SearchIndexError::update(format!(
                    "Add relation failed with status {}: {}",
                    status, error_body
                )));
            }

            debug!(doc_id = %doc_id, relation_id = %relation_id, "Relation added");
            return Ok(());
        }

        // Regular document update with tombstone dominance
        let doc = Self::build_update_doc(request);

        if doc.is_empty() {
            // No fields to update
            return Ok(());
        }

        // Use script with upsert for tombstone dominance:
        // - If document doesn't exist: create it with upsert values
        // - If document exists but is deleted: noop (unless this update sets deleted=true)
        // - If document exists and is not deleted: merge the doc fields
        let mut upsert_doc = doc.clone();
        upsert_doc.insert("entity_id".to_string(), json!(entity_id.to_string()));
        upsert_doc.insert("space_id".to_string(), json!(space_id.to_string()));

        let response = self
            .client
            .update(UpdateParts::IndexId(&self.index_config.alias, &doc_id))
            .body(json!({
                "script": {
                    "source": UPDATE_WITH_TOMBSTONE_CHECK_SCRIPT,
                    "lang": "painless",
                    "params": {
                        "doc": doc
                    }
                },
                "upsert": upsert_doc
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

    /// Unset (remove) specific properties from a document.
    ///
    /// Note: To remove relations, use `EntityOperation::RemoveRelationById` via `bulk_operations`.
    async fn unset_document_properties(
        &self,
        request: &UnsetEntityPropertiesRequest,
    ) -> Result<(), SearchIndexError> {
        // Skip if no property keys to unset
        if request.property_keys.is_empty() {
            return Ok(());
        }

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

    /// Execute multiple operations in bulk, processing them IN ORDER.
    ///
    /// This method handles all operation types (Update, Delete, Unset, RemoveRelationById)
    /// while maintaining the order of operations for consistency.
    ///
    /// For bulk-compatible operations (Update, Delete, Unset), we batch them together.
    /// When we encounter a RemoveRelationById (which requires update_by_query),
    /// we first flush the pending batch, then execute the update_by_query, then continue.
    /// This ensures ordering is preserved.
    #[instrument(skip(self, operations), fields(count = operations.len()))]
    async fn bulk_operations(
        &self,
        operations: &[EntityOperation],
    ) -> Result<BatchOperationSummary, SearchIndexError> {
        if operations.is_empty() {
            return Ok(BatchOperationSummary::empty());
        }

        // Accumulate results across all batches
        let mut all_results: Vec<BatchOperationResult> = Vec::new();
        let mut total_succeeded = 0usize;
        let mut total_failed = 0usize;

        // Current batch of bulk-compatible operations
        let mut bulk_ops: Vec<BulkOperation<Value>> = Vec::new();
        let mut metas: Vec<BulkOperationMeta> = Vec::new();

        for op in operations {
            match op {
                EntityOperation::RemoveRelationById(request) => {
                    debug!(
                        relation_id = %request.relation_id,
                        pending_bulk_ops = bulk_ops.len(),
                        "RemoveRelationById starting — flushing pending bulk ops"
                    );

                    // Flush before executing the update_by_query to maintain ordering
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );

                    // Now execute the update_by_query
                    let relation_uuid =
                        uuid::Uuid::parse_str(&request.relation_id).map_err(|_| {
                            SearchIndexError::validation(format!(
                                "Invalid relation_id: {}",
                                request.relation_id
                            ))
                        })?;

                    // Retry on version conflict (HTTP 409). This handles the
                    // NRT reader race where a preceding bulk write's refresh
                    // hasn't fully propagated to update_by_query's snapshot.
                    const MAX_RETRIES: u32 = 3;
                    const RETRY_DELAY_MS: u64 = 50;
                    let mut attempt = 0u32;

                    loop {
                        let response = self
                            .client
                            .update_by_query(UpdateByQueryParts::Index(&[&self.index_config.alias]))
                            .body(json!({
                                "query": {
                                    "nested": {
                                        "path": "relations",
                                        "query": {
                                            "term": {
                                                "relations.relation_id": relation_uuid.to_string()
                                            }
                                        }
                                    }
                                },
                                "script": {
                                    "source": REMOVE_RELATION_SCRIPT,
                                    "lang": "painless",
                                    "params": {
                                        "relation_id": relation_uuid.to_string()
                                    }
                                }
                            }))
                            .send()
                            .await
                            .map_err(|e| SearchIndexError::update(e.to_string()))?;

                        let status = response.status_code();
                        if status.is_success() {
                            let response_body: Value = response.json().await.unwrap_or_default();
                            let updated = response_body
                                .get("updated")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let total = response_body
                                .get("total")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let conflicts = response_body
                                .get("version_conflicts")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);

                            debug!(
                                relation_id = %relation_uuid,
                                attempt = attempt,
                                updated = updated,
                                total = total,
                                version_conflicts = conflicts,
                                "RemoveRelationById update_by_query completed"
                            );

                            if updated == 0 {
                                warn!(
                                    relation_id = %relation_uuid,
                                    attempt = attempt,
                                    total = total,
                                    version_conflicts = conflicts,
                                    "RemoveRelationById matched 0 documents"
                                );
                            }

                            total_succeeded += 1;
                            all_results.push(BatchOperationResult {
                                entity_id: String::new(),
                                space_id: String::new(),
                                operation_type: "RemoveRelation".to_string(),
                                success: true,
                                error: None,
                            });
                            break;
                        } else if status.as_u16() == 409 && attempt < MAX_RETRIES {
                            attempt += 1;
                            warn!(
                                relation_id = %relation_uuid,
                                attempt = attempt,
                                "Version conflict on RemoveRelationById, retrying"
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(
                                RETRY_DELAY_MS * attempt as u64,
                            ))
                            .await;
                            continue;
                        } else {
                            let error_body = response.text().await.unwrap_or_default();
                            error!(
                                status = %status,
                                body = %error_body,
                                attempt = attempt,
                                "Remove relation by ID failed"
                            );
                            total_failed += 1;
                            all_results.push(BatchOperationResult {
                                entity_id: String::new(),
                                space_id: String::new(),
                                operation_type: "RemoveRelation".to_string(),
                                success: false,
                                error: Some(SearchIndexError::update(format!(
                                    "Remove relation by ID failed: {}",
                                    error_body
                                ))),
                            });
                            break;
                        }
                    }
                }
                EntityOperation::Update(request) => {
                    let (entity_id, space_id) =
                        utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;
                    let doc_id = Self::document_id(&entity_id, &space_id);
                    let mut has_operation = false;

                    // Handle add_relation
                    if let Some(ref relation_data) = request.add_relation {
                        let relation_id = uuid::Uuid::parse_str(&relation_data.relation_id)
                            .map_err(|_| {
                                SearchIndexError::validation(format!(
                                    "Invalid relation_id: {}",
                                    relation_data.relation_id
                                ))
                            })?;
                        let to_entity_id = uuid::Uuid::parse_str(&relation_data.to_entity_id)
                            .map_err(|_| {
                                SearchIndexError::validation(format!(
                                    "Invalid to_entity_id: {}",
                                    relation_data.to_entity_id
                                ))
                            })?;

                        let params = json!({
                            "relation_id": relation_id.to_string(),
                            "relation_type": relation_data.relation_type.clone(),
                            "to_entity_id": to_entity_id.to_string()
                        });

                        let upsert_relation = json!({
                            "relation_id": relation_id.to_string(),
                            "relation_type": relation_data.relation_type.clone(),
                            "to_entity_id": to_entity_id.to_string()
                        });

                        let body = json!({
                            "script": {
                                "source": ADD_RELATION_SCRIPT,
                                "lang": "painless",
                                "params": params
                            },
                            "upsert": {
                                "entity_id": entity_id.to_string(),
                                "space_id": space_id.to_string(),
                                "relations": [upsert_relation]
                            }
                        });
                        debug!(
                            relation_id = %relation_id,
                            relation_type = %relation_data.relation_type,
                            entity_id = %entity_id,
                            space_id = %space_id,
                            to_entity_id = %to_entity_id,
                            pending_bulk_ops = bulk_ops.len(),
                            "Queuing AddRelation bulk op"
                        );
                        bulk_ops.push(BulkOperation::update(doc_id.clone(), body).into());
                        metas.push(BulkOperationMeta {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            operation_type: "AddRelation".to_string(),
                        });
                        has_operation = true;
                    }

                    // Handle regular document properties with tombstone dominance
                    let doc = Self::build_update_doc(request);
                    if !doc.is_empty() {
                        // Use script with upsert for tombstone dominance:
                        // - If document doesn't exist: create it with upsert values
                        // - If document exists but is deleted: noop (unless this update sets deleted=true)
                        // - If document exists and is not deleted: merge the doc fields
                        let mut upsert_doc = doc.clone();
                        upsert_doc.insert("entity_id".to_string(), json!(entity_id.to_string()));
                        upsert_doc.insert("space_id".to_string(), json!(space_id.to_string()));

                        let body = json!({
                            "script": {
                                "source": UPDATE_WITH_TOMBSTONE_CHECK_SCRIPT,
                                "lang": "painless",
                                "params": {
                                    "doc": doc
                                }
                            },
                            "upsert": upsert_doc
                        });
                        bulk_ops.push(BulkOperation::update(doc_id, body).into());
                        metas.push(BulkOperationMeta {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            operation_type: "Update".to_string(),
                        });
                        has_operation = true;
                    }

                    // If no operations, mark as skipped (success)
                    if !has_operation {
                        total_succeeded += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            operation_type: "Update".to_string(),
                            success: true,
                            error: None,
                        });
                    }
                }
                EntityOperation::Delete(request) => {
                    let (entity_id, space_id) =
                        utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;
                    let doc_id = Self::document_id(&entity_id, &space_id);

                    bulk_ops.push(BulkOperation::delete(doc_id).into());
                    metas.push(BulkOperationMeta {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        operation_type: "Delete".to_string(),
                    });
                }
                EntityOperation::Unset(request) => {
                    if request.property_keys.is_empty() {
                        // Skip if nothing to do
                        total_succeeded += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: request.space_id.clone(),
                            operation_type: "Unset".to_string(),
                            success: true,
                            error: None,
                        });
                        continue;
                    }

                    // Flush pending bulk operations before unset to maintain ordering
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );

                    let (entity_id, space_id) =
                        utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;
                    let doc_id = Self::document_id(&entity_id, &space_id);

                    let script_source = create_unset_properties_script(&request.property_keys)?;
                    // Use upsert to handle case where document doesn't exist yet
                    // (can happen when replaying from offset 0 with different batch groupings)
                    let body = json!({
                        "script": {
                            "source": script_source,
                            "lang": "painless"
                        },
                        "upsert": {
                            "entity_id": entity_id.to_string(),
                            "space_id": space_id.to_string()
                        }
                    });
                    bulk_ops.push(BulkOperation::update(doc_id, body).into());
                    metas.push(BulkOperationMeta {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        operation_type: "Unset".to_string(),
                    });

                    // Flush immediately after unset to ensure it's processed before any subsequent
                    // updates to the same document. Multiple updates to the same document in one
                    // bulk request can have undefined behavior in OpenSearch.
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );
                }
                EntityOperation::UpdateEntityGlobalScore(request) => {
                    // Flush before executing the update_by_query to maintain ordering
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );

                    let entity_uuid = uuid::Uuid::parse_str(&request.entity_id).map_err(|_| {
                        SearchIndexError::validation(format!(
                            "Invalid entity_id: {}",
                            request.entity_id
                        ))
                    })?;

                    // Update all documents with this entity_id
                    let response = self
                        .client
                        .update_by_query(UpdateByQueryParts::Index(&[&self.index_config.alias]))
                        .conflicts(Conflicts::Proceed)
                        .body(json!({
                            "query": {
                                "term": {
                                    "entity_id": entity_uuid.to_string()
                                }
                            },
                            "script": {
                                "source": "ctx._source.entity_global_score = params.score",
                                "lang": "painless",
                                "params": {
                                    "score": request.score
                                }
                            }
                        }))
                        .send()
                        .await
                        .map_err(|e| SearchIndexError::update(e.to_string()))?;

                    let status = response.status_code();
                    if status.is_success() {
                        total_succeeded += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: String::new(),
                            operation_type: "UpdateEntityGlobalScore".to_string(),
                            success: true,
                            error: None,
                        });
                        debug!(
                            entity_id = %entity_uuid,
                            score = request.score,
                            "Updated entity global score"
                        );
                    } else {
                        let error_body = response.text().await.unwrap_or_default();
                        error!(status = %status, body = %error_body, "Update entity global score failed");
                        total_failed += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: request.entity_id.clone(),
                            space_id: String::new(),
                            operation_type: "UpdateEntityGlobalScore".to_string(),
                            success: false,
                            error: Some(SearchIndexError::update(format!(
                                "Update entity global score failed: {}",
                                error_body
                            ))),
                        });
                    }
                }
                EntityOperation::UpdateSpaceScore(request) => {
                    // Flush before executing the update_by_query to maintain ordering
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );

                    let space_uuid = uuid::Uuid::parse_str(&request.space_id).map_err(|_| {
                        SearchIndexError::validation(format!(
                            "Invalid space_id: {}",
                            request.space_id
                        ))
                    })?;

                    // Update all documents in this space
                    let response = self
                        .client
                        .update_by_query(UpdateByQueryParts::Index(&[&self.index_config.alias]))
                        .conflicts(Conflicts::Proceed)
                        .body(json!({
                            "query": {
                                "term": {
                                    "space_id": space_uuid.to_string()
                                }
                            },
                            "script": {
                                "source": "ctx._source.space_score = params.score",
                                "lang": "painless",
                                "params": {
                                    "score": request.score
                                }
                            }
                        }))
                        .send()
                        .await
                        .map_err(|e| SearchIndexError::update(e.to_string()))?;

                    let status = response.status_code();
                    if status.is_success() {
                        total_succeeded += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: String::new(),
                            space_id: request.space_id.clone(),
                            operation_type: "UpdateSpaceScore".to_string(),
                            success: true,
                            error: None,
                        });
                        debug!(
                            space_id = %space_uuid,
                            score = request.score,
                            "Updated space score"
                        );
                    } else {
                        let error_body = response.text().await.unwrap_or_default();
                        error!(status = %status, body = %error_body, "Update space score failed");
                        total_failed += 1;
                        all_results.push(BatchOperationResult {
                            entity_id: String::new(),
                            space_id: request.space_id.clone(),
                            operation_type: "UpdateSpaceScore".to_string(),
                            success: false,
                            error: Some(SearchIndexError::update(format!(
                                "Update space score failed: {}",
                                error_body
                            ))),
                        });
                    }
                }
                EntityOperation::UpdateEntitySpaceScore(request) => {
                    // This is a targeted update for a specific document, can be batched
                    let (entity_id, space_id) =
                        utils::parse_entity_and_space_ids(&request.entity_id, &request.space_id)?;
                    let doc_id = Self::document_id(&entity_id, &space_id);

                    let body = json!({
                        "doc": {
                            "entity_id": entity_id.to_string(),
                            "space_id": space_id.to_string(),
                            "entity_space_score": request.score
                        },
                        "doc_as_upsert": true
                    });
                    bulk_ops.push(BulkOperation::update(doc_id, body).into());
                    metas.push(BulkOperationMeta {
                        entity_id: request.entity_id.clone(),
                        space_id: request.space_id.clone(),
                        operation_type: "UpdateEntitySpaceScore".to_string(),
                    });
                }
                EntityOperation::UpdateSpaceTopicEntityId(request) => {
                    // Flush before executing the update_by_query to maintain ordering
                    flush_pending_bulk!(
                        self,
                        bulk_ops,
                        metas,
                        total_succeeded,
                        total_failed,
                        all_results
                    );

                    let space_uuid = uuid::Uuid::parse_str(&request.space_id).map_err(|_| {
                        SearchIndexError::validation(format!(
                            "Invalid space_id: {}",
                            request.space_id
                        ))
                    })?;

                    let topic_entity_uuid = uuid::Uuid::parse_str(&request.topic_entity_id)
                        .map_err(|_| {
                            SearchIndexError::validation(format!(
                                "Invalid topic_entity_id: {}",
                                request.topic_entity_id
                            ))
                        })?;

                    // Retry on version conflict (HTTP 409). Without retries a
                    // conflict would silently skip documents, leaving them with
                    // a stale space_topic_entity_id permanently.
                    const MAX_RETRIES: u32 = 3;
                    const RETRY_DELAY_MS: u64 = 50;
                    let mut attempt = 0u32;

                    loop {
                        let response = self
                            .client
                            .update_by_query(UpdateByQueryParts::Index(&[
                                &self.index_config.alias,
                            ]))
                            .body(json!({
                                "query": {
                                    "term": {
                                        "space_id": space_uuid.to_string()
                                    }
                                },
                                "script": {
                                    "source": "ctx._source.space_topic_entity_id = params.topic_entity_id",
                                    "lang": "painless",
                                    "params": {
                                        "topic_entity_id": topic_entity_uuid.to_string()
                                    }
                                }
                            }))
                            .send()
                            .await
                            .map_err(|e| SearchIndexError::update(e.to_string()))?;

                        let status = response.status_code();
                        if status.is_success() {
                            total_succeeded += 1;
                            all_results.push(BatchOperationResult {
                                entity_id: String::new(),
                                space_id: request.space_id.clone(),
                                operation_type: "UpdateSpaceTopicEntityId".to_string(),
                                success: true,
                                error: None,
                            });
                            debug!(
                                space_id = %space_uuid,
                                topic_entity_id = %topic_entity_uuid,
                                attempt = attempt,
                                "Updated space topic entity ID"
                            );
                            break;
                        } else if status.as_u16() == 409 && attempt < MAX_RETRIES {
                            attempt += 1;
                            warn!(
                                space_id = %space_uuid,
                                attempt = attempt,
                                "Version conflict on UpdateSpaceTopicEntityId, retrying"
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(
                                RETRY_DELAY_MS * attempt as u64,
                            ))
                            .await;
                            continue;
                        } else {
                            let error_body = response.text().await.unwrap_or_default();
                            error!(
                                status = %status,
                                body = %error_body,
                                attempt = attempt,
                                "Update space topic entity ID failed"
                            );
                            total_failed += 1;
                            all_results.push(BatchOperationResult {
                                entity_id: String::new(),
                                space_id: request.space_id.clone(),
                                operation_type: "UpdateSpaceTopicEntityId".to_string(),
                                success: false,
                                error: Some(SearchIndexError::update(format!(
                                    "Update space topic entity ID failed: {}",
                                    error_body
                                ))),
                            });
                            break;
                        }
                    }
                }
            }
        }

        // Flush any remaining bulk operations
        if !bulk_ops.is_empty() {
            let summary = execute_bulk(
                &self.client,
                &self.index_config.alias,
                bulk_ops,
                &metas,
                BulkAction::Update,
                false, // no need to refresh for final batch
            )
            .await?;
            total_succeeded += summary.succeeded;
            total_failed += summary.failed;
            all_results.extend(summary.results);
        }

        Ok(BatchOperationSummary {
            total: operations.len(),
            succeeded: total_succeeded,
            failed: total_failed,
            results: all_results,
        })
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
    fn test_build_update_doc_with_name_only() {
        let request = UpdateEntityRequest {
            entity_id: "entity-1".to_string(),
            space_id: "space-1".to_string(),
            name: Some("Test Name".to_string()),
            description: None,
            avatar: None,
            cover: None,
            image_url: None,
            add_relation: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            deleted: None,
            space_topic_entity_id: None,
        };

        let doc = OpenSearchProvider::build_update_doc(&request);
        assert!(!doc.is_empty());
        assert_eq!(doc.get("name"), Some(&json!("Test Name")));
        assert!(doc.get("description").is_none());
    }

    #[test]
    fn test_build_update_doc_minimal_with_no_optional_properties() {
        let request = UpdateEntityRequest {
            entity_id: "entity-1".to_string(),
            space_id: "space-1".to_string(),
            name: None,
            description: None,
            avatar: None,
            cover: None,
            image_url: None,
            add_relation: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            deleted: None,
            space_topic_entity_id: None,
        };

        let doc = OpenSearchProvider::build_update_doc(&request);
        // Doc should contain only entity_id and space_id (required for upserts)
        assert_eq!(doc.len(), 2);
        assert_eq!(doc.get("entity_id"), Some(&json!("entity-1")));
        assert_eq!(doc.get("space_id"), Some(&json!("space-1")));
        // No optional properties should be present
        assert!(doc.get("name").is_none());
        assert!(doc.get("description").is_none());
    }

    #[test]
    fn test_build_update_doc_ignores_add_relation() {
        use crate::types::RelationData;

        // add_relation is handled separately via script, not included in doc
        let request = UpdateEntityRequest {
            entity_id: "entity-1".to_string(),
            space_id: "space-1".to_string(),
            name: Some("Test Name".to_string()),
            description: None,
            avatar: None,
            cover: None,
            image_url: None,
            add_relation: Some(RelationData {
                relation_id: "rel-1".to_string(),
                relation_type: "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1".to_string(),
                to_entity_id: "type-id-1".to_string(),
            }),
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            deleted: None,
            space_topic_entity_id: None,
        };

        let doc = OpenSearchProvider::build_update_doc(&request);
        // Doc should contain name but NOT add_relation (that's handled via script)
        assert!(!doc.is_empty());
        assert_eq!(doc.get("name"), Some(&json!("Test Name")));
        // add_relation should NOT be in the doc - it's handled separately
        assert!(doc.get("add_relation").is_none());
        assert!(doc.get("relations").is_none());
    }
}
