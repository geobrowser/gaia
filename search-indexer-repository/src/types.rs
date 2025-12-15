//! Request and response types for search index operations.

use crate::errors::SearchIndexError;

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
#[derive(Debug, Clone)]
pub struct UnsetEntityPropertiesRequest {
    /// The entity's unique identifier.
    pub entity_id: String,
    /// The space this entity belongs to.
    pub space_id: String,
    /// The property keys to remove from the document.
    pub property_keys: Vec<String>,
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
