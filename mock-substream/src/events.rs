//! Event types representing raw blockchain events from a substream.
//!
//! These types mirror what the real substream produces, allowing both
//! `hermes-producer` and `atlas` to consume the same mock data.

/// A 16-byte space identifier.
pub type SpaceId = [u8; 16];

/// A 16-byte topic identifier.
pub type TopicId = [u8; 16];

/// A 32-byte address (e.g., Ethereum address or public key).
pub type Address = [u8; 32];

/// A 16-byte edit identifier.
pub type EditId = [u8; 16];

/// A 16-byte entity identifier.
pub type EntityId = [u8; 16];

/// A 16-byte property identifier.
pub type PropertyId = [u8; 16];

/// A 16-byte relation identifier.
pub type RelationId = [u8; 16];

/// A 16-byte relation type identifier.
pub type RelationTypeId = [u8; 16];

/// Metadata about the blockchain state when an event occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockMetadata {
    /// The block number.
    pub block_number: u64,
    /// Unix timestamp in seconds.
    pub block_timestamp: u64,
    /// Transaction hash as a hex string.
    pub tx_hash: String,
    /// Cursor for resuming from this point.
    pub cursor: String,
}

/// A block of events from the mock substream.
#[derive(Debug, Clone)]
pub struct MockBlock {
    /// The block number.
    pub number: u64,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    /// Cursor for resuming from this point.
    pub cursor: String,
    /// Events that occurred in this block.
    pub events: Vec<MockEvent>,
}

/// Events that can occur on-chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockEvent {
    /// A new space was created.
    SpaceCreated(SpaceCreated),
    /// Trust was extended from one space to another.
    TrustExtended(TrustExtended),
    /// An edit was published to a space.
    EditPublished(EditPublished),
}

/// Event emitted when a new space is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceCreated {
    /// Metadata about the block this event occurred in.
    pub meta: BlockMetadata,
    /// The unique identifier of the new space.
    pub space_id: SpaceId,
    /// The topic ID associated with this space.
    pub topic_id: TopicId,
    /// The type of space (personal or DAO).
    pub space_type: SpaceType,
}

/// The type of a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceType {
    /// A personal space owned by a single address.
    Personal {
        /// The owner's address.
        owner: Address,
    },
    /// A DAO space with multiple editors and members.
    Dao {
        /// Initial editors who can modify the space.
        initial_editors: Vec<SpaceId>,
        /// Initial members of the DAO.
        initial_members: Vec<SpaceId>,
    },
}

/// Event emitted when trust is extended from one space to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustExtended {
    /// Metadata about the block this event occurred in.
    pub meta: BlockMetadata,
    /// The space extending trust.
    pub source_space_id: SpaceId,
    /// The type of trust extension.
    pub extension: TrustExtension,
}

/// The type of trust extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustExtension {
    /// The source space verifies the target space.
    Verified {
        /// The space being verified.
        target_space_id: SpaceId,
    },
    /// The source space marks the target as related.
    Related {
        /// The related space.
        target_space_id: SpaceId,
    },
    /// The source space marks a topic as a subtopic.
    Subtopic {
        /// The subtopic's topic ID.
        target_topic_id: TopicId,
    },
}

/// Event emitted when an edit is published to a space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPublished {
    /// Metadata about the block this event occurred in.
    pub meta: BlockMetadata,
    /// The unique identifier of this edit.
    pub edit_id: EditId,
    /// The space this edit belongs to.
    pub space_id: SpaceId,
    /// The authors of this edit.
    pub authors: Vec<Address>,
    /// The name/description of this edit.
    pub name: String,
    /// The GRC-20 operations in this edit.
    pub ops: Vec<Op>,
}

/// A GRC-20 operation.
///
/// These operations mirror the wire/grc20 protobuf definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Create or update an entity with values.
    UpdateEntity(UpdateEntity),
    /// Create a relation between entities.
    CreateRelation(CreateRelation),
    /// Update an existing relation.
    UpdateRelation(UpdateRelation),
    /// Delete a relation by ID.
    DeleteRelation(RelationId),
    /// Create/define a property type.
    CreateProperty(CreateProperty),
    /// Unset values on an entity.
    UnsetEntityValues(UnsetEntityValues),
    /// Unset fields on a relation.
    UnsetRelationFields(UnsetRelationFields),
}

/// Operation to create or update an entity with values.
///
/// Maps to `wire::pb::grc20::Entity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateEntity {
    /// The entity ID.
    pub id: EntityId,
    /// Property values to set on this entity.
    pub values: Vec<Value>,
}

/// A property value on an entity.
///
/// Maps to `wire::pb::grc20::Value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    /// The property ID this value is for.
    pub property: PropertyId,
    /// The value as a string.
    pub value: String,
}

/// Operation to create a relation between entities.
///
/// Maps to `wire::pb::grc20::Relation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRelation {
    /// The relation ID.
    pub id: RelationId,
    /// The relation type ID.
    pub relation_type: RelationTypeId,
    /// The source entity ID.
    pub from_entity: EntityId,
    /// The source space ID (optional).
    pub from_space: Option<SpaceId>,
    /// The target entity ID.
    pub to_entity: EntityId,
    /// The target space ID (optional).
    pub to_space: Option<SpaceId>,
    /// The relation entity ID (for storing relation properties).
    pub entity: EntityId,
    /// Position in an ordered list (optional).
    pub position: Option<String>,
    /// Whether this relation is verified.
    pub verified: Option<bool>,
}

/// Operation to update an existing relation.
///
/// Maps to `wire::pb::grc20::RelationUpdate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRelation {
    /// The relation ID to update.
    pub id: RelationId,
    /// New source space (optional).
    pub from_space: Option<SpaceId>,
    /// New target space (optional).
    pub to_space: Option<SpaceId>,
    /// New position (optional).
    pub position: Option<String>,
    /// New verified status (optional).
    pub verified: Option<bool>,
}

/// Operation to create/define a property type.
///
/// Maps to `wire::pb::grc20::Property`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProperty {
    /// The property ID.
    pub id: PropertyId,
    /// The data type of this property.
    pub data_type: DataType,
}

/// Data types for properties.
///
/// Maps to `wire::pb::grc20::DataType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    String = 0,
    Number = 1,
    Boolean = 2,
    Time = 3,
    Point = 4,
    Relation = 5,
}

/// Operation to unset values on an entity.
///
/// Maps to `wire::pb::grc20::UnsetEntityValues`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsetEntityValues {
    /// The entity ID.
    pub id: EntityId,
    /// The property IDs to unset.
    pub properties: Vec<PropertyId>,
}

/// Operation to unset fields on a relation.
///
/// Maps to `wire::pb::grc20::UnsetRelationFields`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsetRelationFields {
    /// The relation ID.
    pub id: RelationId,
    /// Whether to unset from_space.
    pub from_space: Option<bool>,
    /// Whether to unset to_space.
    pub to_space: Option<bool>,
    /// Whether to unset position.
    pub position: Option<bool>,
    /// Whether to unset verified.
    pub verified: Option<bool>,
}

/// Helper to create a well-known ID from a single byte.
///
/// Creates an ID with all zeros except the last byte.
/// Example: `make_id(0x0A)` produces `[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x0A]`
pub const fn make_id(last_byte: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last_byte]
}

/// Helper to create a well-known address from a single byte.
///
/// Creates an address with all zeros except the last byte.
pub const fn make_address(last_byte: u8) -> Address {
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, last_byte,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_id() {
        let id = make_id(0x0A);
        assert_eq!(id[15], 0x0A);
        assert!(id[..15].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_make_address() {
        let addr = make_address(0xFF);
        assert_eq!(addr[31], 0xFF);
        assert!(addr[..31].iter().all(|&b| b == 0));
    }
}
