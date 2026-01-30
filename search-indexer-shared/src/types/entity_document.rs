//! Entity document types for the search index.
//!
//! This module defines the document structure that is indexed in the search engine.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A type relation entry stored in the entity document.
///
/// This tracks "type" relations associated with this entity, allowing:
/// - Filtering entities by type using nested queries on `entity_to_id`
/// - Looking up and removing relations by their `relation_id` when a DeleteRelation event occurs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeRelationEntry {
    /// The relation's unique identifier.
    pub relation_id: Uuid,
    /// The entity this relation points to (the "type" entity).
    pub entity_to_id: Uuid,
}

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
/// - `type_relations`: List of type relations (used for filtering entities by type via nested queries)
/// - `entity_global_score`: Global reputation score (None until scoring service)
/// - `space_score`: Space-level score (None until scoring service)
/// - `entity_space_score`: Entity's score within the space (None until scoring service)
/// - `indexed_at`: Timestamp when the document was indexed
/// - `deleted`: Optional soft delete flag (None or Some(true) when deleted)
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
    /// Type relations associated with this entity (for filtering by type_id via nested queries).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_relations: Vec<TypeRelationEntry>,
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
    /// Soft delete flag - None for active entities, Some(true) for deleted entities
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
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
            type_relations: Vec::new(),
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            indexed_at: Utc::now(),
            deleted: None,
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
            type_relations: Vec::new(),
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            indexed_at: Utc::now(),
            deleted: None,
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
        assert!(doc.type_relations.is_empty());
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

    #[test]
    fn test_type_relations_serialization() {
        let mut doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
        );

        let type_relations = vec![
            TypeRelationEntry {
                relation_id: Uuid::new_v4(),
                entity_to_id: Uuid::new_v4(),
            },
            TypeRelationEntry {
                relation_id: Uuid::new_v4(),
                entity_to_id: Uuid::new_v4(),
            },
        ];
        doc.type_relations = type_relations.clone();

        let json = serde_json::to_string(&doc).unwrap();
        let deserialized: EntityDocument = serde_json::from_str(&json).unwrap();

        assert_eq!(doc.entity_id, deserialized.entity_id);
        assert_eq!(doc.space_id, deserialized.space_id);
        assert_eq!(doc.name, deserialized.name);
        assert_eq!(doc.description, deserialized.description);
        assert_eq!(doc.type_relations, type_relations);
    }

    #[test]
    fn test_empty_type_relations_not_serialized() {
        let doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
        );

        // type_relations is empty by default
        assert!(doc.type_relations.is_empty());

        let json = serde_json::to_string(&doc).unwrap();
        // The type_relations field should not appear in the JSON when empty due to skip_serializing_if
        assert!(!json.contains("type_relations"));

        let deserialized: EntityDocument = serde_json::from_str(&json).unwrap();
        assert!(deserialized.type_relations.is_empty());
    }

    #[test]
    fn test_type_relations_with_other_optional_fields() {
        let mut doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Description".to_string()),
        );

        let type_relations = vec![TypeRelationEntry {
            relation_id: Uuid::new_v4(),
            entity_to_id: Uuid::new_v4(),
        }];
        doc.type_relations = type_relations.clone();
        doc.avatar = Some("avatar.jpg".to_string());
        doc.cover = Some("cover.jpg".to_string());
        doc.entity_global_score = Some(0.85);
        doc.space_score = Some(0.92);
        doc.entity_space_score = Some(0.78);

        let json = serde_json::to_string(&doc).unwrap();
        let deserialized: EntityDocument = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.type_relations, type_relations);
        assert_eq!(deserialized.avatar, Some("avatar.jpg".to_string()));
        assert_eq!(deserialized.cover, Some("cover.jpg".to_string()));
        assert_eq!(deserialized.entity_global_score, Some(0.85));
        assert_eq!(deserialized.space_score, Some(0.92));
        assert_eq!(deserialized.entity_space_score, Some(0.78));
    }

    #[test]
    fn test_type_relations_roundtrip_serialization() {
        let type_relations = vec![
            TypeRelationEntry {
                relation_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
                entity_to_id: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap(),
            },
            TypeRelationEntry {
                relation_id: Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
                entity_to_id: Uuid::parse_str("87654321-4321-4321-4321-cba987654321").unwrap(),
            },
        ];

        let mut doc = EntityDocument::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some("Test Entity".to_string()),
            Some("Test Description".to_string()),
        );
        doc.type_relations = type_relations.clone();

        // Serialize to JSON
        let json = serde_json::to_string(&doc).unwrap();

        // Deserialize back
        let deserialized: EntityDocument = serde_json::from_str(&json).unwrap();

        // Verify type_relations are preserved exactly
        assert_eq!(deserialized.type_relations, type_relations);
        assert_eq!(deserialized.type_relations.len(), 2);

        // Verify all UUIDs are correct
        assert_eq!(
            deserialized.type_relations[0].relation_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(
            deserialized.type_relations[0].entity_to_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap()
        );
    }
}
