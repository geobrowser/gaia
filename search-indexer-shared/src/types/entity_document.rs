//! Entity document types for the search index.
//!
//! This module defines the document structure that is indexed in the search engine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Document representation for the search index.
///
/// This struct represents an entity as it is stored in the search engine.
/// Scores default to `None` - they will be populated by the scoring service
/// in a future version.
///
/// # Fields
///
/// - `entity_id`: Unique identifier for the entity
/// - `space_id`: The space this entity belongs to
/// - `name`: Optional entity display name (primary search field)
/// - `description`: Optional description text (secondary search field)
/// - `avatar`: Optional avatar image URL
/// - `cover`: Optional cover image URL
/// - `entity_global_score`: Global reputation score (None until scoring service)
/// - `space_score`: Space-level score (None until scoring service)
/// - `entity_space_score`: Entity's score within the space (None until scoring service)
/// - `indexed_at`: Timestamp when the document was indexed
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityDocument {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// Global entity score - None until scoring service is implemented
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_global_score: Option<f64>,
    /// Space score - None until scoring service is implemented
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_score: Option<f64>,
    /// Entity-space score - None until scoring service is implemented
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_space_score: Option<f64>,
    pub indexed_at: DateTime<Utc>,
}

impl EntityDocument {
    /// Create a new document with default `None` scores.
    ///
    /// # Arguments
    ///
    /// * `entity_id` - The unique identifier for the entity
    /// * `space_id` - The space this entity belongs to
    /// * `name` - Optional entity display name
    /// * `description` - Optional description text
    ///
    /// # Example
    ///
    /// ```
    /// use search_indexer_shared::EntityDocument;
    /// use uuid::Uuid;
    ///
    /// let doc = EntityDocument::new(
    ///     Uuid::new_v4(),
    ///     Uuid::new_v4(),
    ///     Some("My Entity".to_string()),
    ///     Some("A description".to_string()),
    /// );
    /// ```
    pub fn new(
        entity_id: Uuid,
        space_id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            entity_id,
            space_id,
            name,
            description,
            avatar: None,
            cover: None,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            indexed_at: Utc::now(),
        }
    }

    /// Create a new document with all optional fields.
    ///
    /// # Arguments
    ///
    /// * `entity_id` - The unique identifier for the entity
    /// * `space_id` - The space this entity belongs to
    /// * `name` - Optional entity display name
    /// * `description` - Optional description text
    /// * `avatar` - Optional avatar image URL
    /// * `cover` - Optional cover image URL
    #[allow(clippy::too_many_arguments)]
    pub fn with_images(
        entity_id: Uuid,
        space_id: Uuid,
        name: Option<String>,
        description: Option<String>,
        avatar: Option<String>,
        cover: Option<String>,
    ) -> Self {
        Self {
            entity_id,
            space_id,
            name,
            description,
            avatar,
            cover,
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            indexed_at: Utc::now(),
        }
    }

    /// Generate the document ID used in the search index.
    ///
    /// The document ID is a combination of entity_id and space_id to ensure
    /// uniqueness across spaces.
    pub fn document_id(&self) -> String {
        format!("{}_{}", self.entity_id, self.space_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_document_new() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();
        let name = Some("Test Entity".to_string());
        let description = Some("Test description".to_string());

        let doc = EntityDocument::new(entity_id, space_id, name.clone(), description.clone());

        assert_eq!(doc.entity_id, entity_id);
        assert_eq!(doc.space_id, space_id);
        assert_eq!(doc.name, name);
        assert_eq!(doc.description, description);
        assert!(doc.avatar.is_none());
        assert!(doc.cover.is_none());
        assert!(doc.entity_global_score.is_none());
        assert!(doc.space_score.is_none());
        assert!(doc.entity_space_score.is_none());
    }

    #[test]
    fn test_entity_document_new_without_name() {
        let entity_id = Uuid::new_v4();
        let space_id = Uuid::new_v4();

        let doc = EntityDocument::new(entity_id, space_id, None, None);

        assert_eq!(doc.entity_id, entity_id);
        assert_eq!(doc.space_id, space_id);
        assert!(doc.name.is_none());
        assert!(doc.description.is_none());
    }

    #[test]
    fn test_document_id() {
        let entity_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let space_id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();

        let doc = EntityDocument::new(entity_id, space_id, Some("Test".to_string()), None);

        assert_eq!(
            doc.document_id(),
            "550e8400-e29b-41d4-a716-446655440000_6ba7b810-9dad-11d1-80b4-00c04fd430c8"
        );
    }

    #[test]
    fn test_serialization() {
        let doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test".to_string()),
            None,
        );

        let json = serde_json::to_string(&doc).unwrap();
        let deserialized: EntityDocument = serde_json::from_str(&json).unwrap();

        assert_eq!(doc.entity_id, deserialized.entity_id);
        assert_eq!(doc.name, deserialized.name);
    }
}
