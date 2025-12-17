//! Message types for the consumer.
//!
//! Defines the event structures that flow through the ingest.

use uuid::Uuid;

/// Types of entity events that can be received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityEventType {
    /// Entity was created or updated.
    Upsert,
    /// Entity was deleted.
    Delete,
    /// Properties were unset from an entity.
    UnsetProperties,
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
        }
    }
}

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
