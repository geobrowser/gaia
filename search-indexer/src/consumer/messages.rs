//! Message types for the consumer.
//!
//! Defines the event structures that flow through the ingest.

use uuid::Uuid;

// ============================================================================
// Entity Events - from knowledge.edits Kafka topic
// ============================================================================

/// Types of entity events that can be received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityEventType {
    /// Entity was created or updated.
    Upsert,
    /// Entity was deleted.
    Delete,
    /// Properties were unset from an entity.
    UnsetProperties,
    /// Relation was created, which may affect entity's type_relations.
    CreateRelation,
    /// Relation was deleted, which may affect entity's type_relations.
    DeleteRelation,
}

/// An entity event received from Kafka.
#[derive(Debug, Clone)]
pub struct EntityEvent {
    /// The type of event.
    pub event_type: EntityEventType,
    /// The entity's unique identifier.
    pub entity_id: Uuid,
    /// The space this entity belongs to.
    pub space_id: Uuid,
    /// The entity's name (for upsert events).
    pub name: Option<String>,
    /// The entity's description (for upsert events).
    pub description: Option<String>,
    /// Avatar URL (for upsert events).
    pub avatar: Option<String>,
    /// Cover image URL (for upsert events).
    pub cover: Option<String>,
    /// Property keys to unset (for unset_properties events).
    pub unset_property_keys: Vec<String>,
    /// Relation ID (for relation events).
    pub relation_id: Option<Uuid>,
    /// Relation type (for relation events).
    pub relation_type: Option<Uuid>,
    /// To entity ID (for relation events).
    pub to_entity_id: Option<Uuid>,
}

impl EntityEvent {
    /// Create a new upsert event.
    pub fn upsert(
        entity_id: Uuid,
        space_id: Uuid,
        name: Option<String>,
        description: Option<String>,
        avatar: Option<String>,
    ) -> Self {
        Self {
            event_type: EntityEventType::Upsert,
            entity_id,
            space_id,
            name,
            description,
            avatar,
            cover: None, // Cover will be used in the future
            unset_property_keys: Vec::new(),
            relation_id: None,
            relation_type: None,
            to_entity_id: None,
        }
    }

    /// Create a new delete event.
    pub fn delete(entity_id: Uuid, space_id: Uuid) -> Self {
        Self {
            event_type: EntityEventType::Delete,
            entity_id,
            space_id,
            name: None,
            description: None,
            avatar: None,
            cover: None, // Cover will be used in the future
            unset_property_keys: Vec::new(),
            relation_id: None,
            relation_type: None,
            to_entity_id: None,
        }
    }

    /// Create a new unset properties event.
    pub fn unset_properties(entity_id: Uuid, space_id: Uuid, property_keys: Vec<String>) -> Self {
        Self {
            event_type: EntityEventType::UnsetProperties,
            entity_id,
            space_id,
            name: None,
            description: None,
            avatar: None,
            cover: None, // Cover will be used in the future
            unset_property_keys: property_keys,
            relation_id: None,
            relation_type: None,
            to_entity_id: None,
        }
    }

    /// Create a new create relation event.
    pub fn create_relation(
        relation_id: Uuid,
        relation_type: Uuid,
        entity_id: Uuid,
        to_entity_id: Uuid,
        space_id: Uuid,
    ) -> Self {
        Self {
            event_type: EntityEventType::CreateRelation,
            entity_id, // The entity whose type_relations may be affected
            space_id,
            name: None,
            description: None,
            avatar: None,
            cover: None,
            unset_property_keys: Vec::new(),
            relation_id: Some(relation_id),
            relation_type: Some(relation_type),
            to_entity_id: Some(to_entity_id),
        }
    }

    /// Create a new delete relation event.
    ///
    /// Only the relation_id is available when processing DeleteRelation Kafka messages.
    pub fn delete_relation(relation_id: Uuid) -> Self {
        Self {
            event_type: EntityEventType::DeleteRelation,
            // These fields are not used when only relation_id is available
            entity_id: Uuid::nil(),
            space_id: Uuid::nil(),
            name: None,
            description: None,
            avatar: None,
            cover: None,
            unset_property_keys: Vec::new(),
            relation_id: Some(relation_id),
            relation_type: None, // No relation type info available
            to_entity_id: None,  // No entity info available
        }
    }
}

// ============================================================================
// Score Events - from curation.scores Kafka topic
// ============================================================================

/// Score update event types.
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreEventType {
    /// Update an entity's global score.
    EntityGlobalScore,
    /// Update a space's score.
    SpaceScore,
    /// Update an entity's score within a specific space (perspective score).
    EntitySpaceScore,
}

/// A score update event received from the curation.scores Kafka topic.
#[derive(Debug, Clone)]
pub struct ScoreEvent {
    /// The type of score update.
    pub event_type: ScoreEventType,
    /// Entity ID (for EntityGlobalScore and EntitySpaceScore).
    pub entity_id: Option<Uuid>,
    /// Space ID (for SpaceScore and EntitySpaceScore).
    pub space_id: Option<Uuid>,
    /// The score value.
    pub score: f64,
    /// When the score was last updated (Unix timestamp in seconds).
    pub updated_at: u64,
}

impl ScoreEvent {
    /// Create a new entity global score event.
    pub fn entity_global_score(entity_id: Uuid, score: f64, updated_at: u64) -> Self {
        Self {
            event_type: ScoreEventType::EntityGlobalScore,
            entity_id: Some(entity_id),
            space_id: None,
            score,
            updated_at,
        }
    }

    /// Create a new space score event.
    pub fn space_score(space_id: Uuid, score: f64, updated_at: u64) -> Self {
        Self {
            event_type: ScoreEventType::SpaceScore,
            entity_id: None,
            space_id: Some(space_id),
            score,
            updated_at,
        }
    }

    /// Create a new entity space score (perspective) event.
    pub fn entity_space_score(
        entity_id: Uuid,
        space_id: Uuid,
        score: f64,
        updated_at: u64,
    ) -> Self {
        Self {
            event_type: ScoreEventType::EntitySpaceScore,
            entity_id: Some(entity_id),
            space_id: Some(space_id),
            score,
            updated_at,
        }
    }
}

// ============================================================================
// Stream Messages - internal message passing
// ============================================================================

/// Messages that flow through the ingest.
#[derive(Debug)]
pub enum StreamMessage {
    /// A batch of entity events with associated offsets for acknowledgment.
    Events {
        events: Vec<EntityEvent>,
        offsets: Vec<(String, i32, i64)>,
    },
    /// Acknowledgment that events were successfully processed.
    Acknowledgment {
        offsets: Vec<(String, i32, i64)>,
        success: bool,
        error: Option<String>,
    },
    /// Stream has ended.
    End,
    /// An error occurred.
    Error(String),
}
