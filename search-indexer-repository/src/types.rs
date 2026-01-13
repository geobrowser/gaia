//! Request and response types for search index operations.

use crate::errors::SearchIndexError;

/// Data for a type relation to be added to an entity document.
///
/// This struct contains all the information needed to add a type relation entry
/// to an entity's `type_relations` array.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRelationData {
    /// The relation's unique identifier.
    pub relation_id: String,
    /// The entity this relation points to (the "type" entity).
    pub entity_to_id: String,
}

/// Request to update an existing entity document in the search index.
///
/// This struct allows partial updates to an entity document. The `entity_id` and
/// `space_id` are required to identify the document. Only fields that are `Some`
/// will be updated; fields that are `None` will remain unchanged in the index.
#[derive(Debug, Clone)]
pub struct UpdateEntityRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space this entity belongs to.
    pub space_id: String,
    /// The entity's display name.
    pub name: Option<String>,
    /// Optional description text.
    pub description: Option<String>,
    /// Optional avatar image URL.
    pub avatar: Option<String>,
    /// Optional cover image URL.
    pub cover: Option<String>,
    /// Atomically add a type relation to the entity's type_relations array.
    /// Does not overwrite existing data.
    pub add_type_relation: Option<TypeRelationData>,
    /// Global entity score.
    pub entity_global_score: Option<f64>,
    /// Space score.
    pub space_score: Option<f64>,
    /// Entity-space score.
    pub entity_space_score: Option<f64>,
}

/// Request to delete an entity document from the search index.
///
/// This struct identifies the document to delete using `entity_id` and `space_id`.
/// Both fields are required and must be valid UUIDs.
#[derive(Debug, Clone)]
pub struct DeleteEntityRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space this entity belongs to.
    pub space_id: String,
}

/// Request to unset (remove) specific properties from an entity document.
///
/// This struct allows removing specific fields from a document. The `entity_id` and
/// `space_id` are required to identify the document. The `property_keys` vector
/// contains the names of the fields to remove (e.g., "name", "description", "avatar", "cover").
///
/// Note: To remove type relations, use `EntityOperation::RemoveTypeRelationById` instead.
#[derive(Debug, Clone)]
pub struct UnsetEntityPropertiesRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space this entity belongs to.
    pub space_id: String,
    /// The property keys to remove from the document.
    pub property_keys: Vec<String>,
}

/// Data for removing a type relation from an entity document by relation_id.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveTypeRelationData {
    /// The relation's unique identifier to remove.
    pub relation_id: String,
}

/// Request to update an entity's global score across all spaces.
///
/// This will update the `entity_global_score` field for ALL documents
/// that have the given entity_id (one update per space the entity exists in).
#[derive(Debug, Clone)]
pub struct UpdateEntityGlobalScoreRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The new global score value.
    pub score: f64,
}

/// Request to update a space's score across all entities in that space.
///
/// This will update the `space_score` field for ALL documents
/// that have the given space_id.
#[derive(Debug, Clone)]
pub struct UpdateSpaceScoreRequest {
    /// The space's unique identifier.
    pub space_id: String,
    /// The new space score value.
    pub score: f64,
}

/// Request to update an entity's score within a specific space.
///
/// This is the most targeted score update - affects exactly one document
/// identified by the entity_id + space_id combination.
#[derive(Debug, Clone)]
pub struct UpdateEntitySpaceScoreRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space's unique identifier.
    pub space_id: String,
    /// The new entity-space score value.
    pub score: f64,
}

/// A single operation in a bulk request.
///
/// This enum represents any operation that can be performed on an entity document.
/// Operations are processed in order, maintaining consistency for operations on the same entity.
#[derive(Debug, Clone)]
pub enum EntityOperation {
    /// Update/upsert an entity document.
    Update(UpdateEntityRequest),
    /// Delete an entity document.
    Delete(DeleteEntityRequest),
    /// Unset specific properties from an entity document.
    Unset(UnsetEntityPropertiesRequest),
    /// Remove a type relation by relation_id (searches for documents containing it).
    /// Used when only the relation_id is available (e.g., from DeleteRelation Kafka messages).
    RemoveTypeRelationById(RemoveTypeRelationData),
    /// Update an entity's global score across all spaces.
    /// Uses update_by_query to update all documents with this entity_id.
    UpdateEntityGlobalScore(UpdateEntityGlobalScoreRequest),
    /// Update a space's score across all entities in that space.
    /// Uses update_by_query to update all documents in this space.
    UpdateSpaceScore(UpdateSpaceScoreRequest),
    /// Update an entity's score within a specific space.
    /// Uses a targeted update for the specific entity+space document.
    UpdateEntitySpaceScore(UpdateEntitySpaceScoreRequest),
}

impl EntityOperation {
    /// Get the entity_id for this operation.
    /// Returns an empty string for operations that don't target a specific entity.
    pub fn entity_id(&self) -> &str {
        match self {
            EntityOperation::Update(r) => &r.entity_id,
            EntityOperation::Delete(r) => &r.entity_id,
            EntityOperation::Unset(r) => &r.entity_id,
            EntityOperation::RemoveTypeRelationById(_) => "",
            EntityOperation::UpdateEntityGlobalScore(r) => &r.entity_id,
            EntityOperation::UpdateSpaceScore(_) => "",
            EntityOperation::UpdateEntitySpaceScore(r) => &r.entity_id,
        }
    }

    /// Get the space_id for this operation.
    /// Returns an empty string for operations that don't target a specific space.
    pub fn space_id(&self) -> &str {
        match self {
            EntityOperation::Update(r) => &r.space_id,
            EntityOperation::Delete(r) => &r.space_id,
            EntityOperation::Unset(r) => &r.space_id,
            EntityOperation::RemoveTypeRelationById(_) => "",
            EntityOperation::UpdateEntityGlobalScore(_) => "",
            EntityOperation::UpdateSpaceScore(r) => &r.space_id,
            EntityOperation::UpdateEntitySpaceScore(r) => &r.space_id,
        }
    }
}

/// Result of a batch operation for a single item.
///
/// This struct represents the outcome of a single operation within a batch (e.g.,
/// indexing, updating, or deleting one document). It indicates whether the operation
/// succeeded and includes error details if it failed.
#[derive(Debug, Clone)]
pub struct BatchOperationResult {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space this entity belongs to.
    pub space_id: String,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Error if the operation failed.
    pub error: Option<SearchIndexError>,
}

/// Summary of a batch operation containing aggregate statistics and individual results.
///
/// This struct provides a complete overview of a bulk operation, including the total
/// number of items processed, how many succeeded and failed, and detailed results for
/// each individual item. This allows callers to handle partial failures gracefully.
#[derive(Debug, Clone)]
pub struct BatchOperationSummary {
    /// Total number of items in the batch.
    pub total: usize,
    /// Number of successful operations.
    pub succeeded: usize,
    /// Number of failed operations.
    pub failed: usize,
    /// Individual results for each item.
    pub results: Vec<BatchOperationResult>,
}
