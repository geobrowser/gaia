//! Conversion utilities for transforming Hermes protobuf messages to ActionRaw.

use actions_indexer_shared::types::{ActionRaw, ActionType, ObjectType};
use alloy::primitives::{Bytes, TxHash};
use hermes_schema::pb::voting::{HermesVoteCast, VoteDirection};
use uuid::Uuid;

use crate::errors::{ConsumerError, ConversionError};

/// ABI-encoded data field size: 96 bytes
/// Format: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
/// - bytes 0-31: uint16 version (right-aligned, value in bytes 30-31)
/// - bytes 32-63: bytes16 groupId (left-aligned, value in bytes 32-47)
/// - bytes 64-95: bytes16 spacePOV (left-aligned, value in bytes 64-79)
const ABI_ENCODED_DATA_SIZE: usize = 96;

/// Converts a HermesVoteCast protobuf message to an ActionRaw struct.
///
/// # Field Mappings
/// - `voter_id` (16 bytes) → `user_id` (UUID)
/// - `object_type` (4 bytes) → `object_type` (Entity/Relation discriminator)
/// - `object_id` (16 bytes) → `object_id` (UUID)
/// - `direction` → encoded in `metadata` (0=Up, 1=Down, 2=Remove)
/// - `data` → ABI-encoded: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
///   - version → `action_version`
///   - groupId → `group_id`
///   - spacePOV → `space_pov`
/// - `meta.block_number` → `block_number`
/// - `meta.created_at` → `block_timestamp`
/// - `meta.cursor` → used for offset tracking (not stored in ActionRaw)
///
/// # Vote Direction Encoding
/// The vote direction is encoded as the first byte of the metadata field:
/// - 0 = Upvote
/// - 1 = Downvote
/// - 2 = Remove (unvote)
///
/// # Arguments
/// * `vote` - The HermesVoteCast protobuf message to convert
///
/// # Returns
/// * `Ok(ActionRaw)` - Successfully converted action
/// * `Err(ConsumerError)` - Conversion failed due to invalid data
pub fn hermes_vote_to_action_raw(vote: &HermesVoteCast) -> Result<ActionRaw, ConsumerError> {
    // Parse voter_id (16 bytes UUID)
    let user_id = bytes_to_uuid(&vote.voter_id, "voter_id")?;

    // Parse object_id (16 bytes UUID)
    let object_id = bytes_to_uuid(&vote.object_id, "object_id")?;

    // Parse object_type (4 bytes discriminator)
    let object_type = parse_object_type(&vote.object_type)?;

    // Get blockchain metadata
    let meta = vote
        .meta
        .as_ref()
        .ok_or_else(|| ConversionError::MissingField("meta".to_string()))?;

    // Parse the ABI-encoded data field to extract version, groupId, and spacePOV
    let (action_version, group_id, space_pov) = parse_vote_data(&vote.data)?;

    // Encode vote direction as first byte of metadata
    let vote_direction_byte = match VoteDirection::try_from(vote.direction) {
        Ok(VoteDirection::Up) => 0u8,
        Ok(VoteDirection::Down) => 1u8,
        Ok(VoteDirection::None) => 2u8, // Remove/unvote
        Err(_) => {
            return Err(ConversionError::InvalidVoteDirection(format!(
                "unknown direction: {}",
                vote.direction
            ))
            .into());
        }
    };

    // Build metadata: just the vote direction byte
    let metadata = Some(Bytes::from(vec![vote_direction_byte]));

    // For Kafka events, we use a placeholder tx_hash since blockchain tx info
    // may not be directly available. The cursor in meta serves as the unique identifier.
    let tx_hash = TxHash::ZERO;

    Ok(ActionRaw {
        action_type: ActionType::Vote,
        action_version,
        user_id,
        object_id,
        group_id: Some(group_id),
        space_pov,
        metadata,
        block_number: meta.block_number,
        block_timestamp: meta.created_at,
        tx_hash,
        object_type,
    })
}

/// Parses the ABI-encoded data field from HermesVoteCast.
///
/// The data field is encoded as: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
///
/// ABI encoding layout (96 bytes total):
/// - bytes 0-31: uint16 version (right-aligned, value in bytes 30-31)
/// - bytes 32-63: bytes16 groupId (left-aligned, value in bytes 32-47)  
/// - bytes 64-95: bytes16 spacePOV (left-aligned, value in bytes 64-79)
///
/// # Arguments
/// * `data` - The ABI-encoded data bytes
///
/// # Returns
/// * `Ok((version, group_id, space_pov))` - Successfully parsed values
/// * `Err(ConsumerError)` - Invalid data format
fn parse_vote_data(data: &[u8]) -> Result<(u64, Uuid, Uuid), ConsumerError> {
    if data.len() != ABI_ENCODED_DATA_SIZE {
        return Err(ConversionError::InvalidDataField(format!(
            "expected {} bytes for ABI-encoded data, got {}",
            ABI_ENCODED_DATA_SIZE,
            data.len()
        ))
        .into());
    }

    // Extract version from bytes 30-31 (uint16, big-endian in ABI encoding)
    let version_bytes: [u8; 2] = data[30..32]
        .try_into()
        .map_err(|_| ConversionError::InvalidDataField("failed to read version bytes".to_string()))?;
    let action_version = u16::from_be_bytes(version_bytes) as u64;

    // Extract groupId from bytes 32-47 (bytes16, left-aligned)
    let group_id = bytes_to_uuid(&data[32..48], "groupId")?;

    // Extract spacePOV from bytes 64-79 (bytes16, left-aligned)
    let space_pov = bytes_to_uuid(&data[64..80], "spacePOV")?;

    Ok((action_version, group_id, space_pov))
}

/// Converts a byte slice to a UUID.
///
/// # Arguments
/// * `bytes` - The byte slice (must be exactly 16 bytes)
/// * `field_name` - Name of the field for error messages
///
/// # Returns
/// * `Ok(Uuid)` - Successfully parsed UUID
/// * `Err(ConsumerError)` - Invalid byte length
fn bytes_to_uuid(bytes: &[u8], field_name: &str) -> Result<Uuid, ConsumerError> {
    if bytes.len() != 16 {
        return Err(ConversionError::InvalidUuid(format!(
            "{}: expected 16 bytes, got {}",
            field_name,
            bytes.len()
        ))
        .into());
    }

    let bytes_array: [u8; 16] = bytes.try_into().map_err(|_| {
        ConversionError::InvalidUuid(format!("{}: failed to convert to array", field_name))
    })?;

    Ok(Uuid::from_bytes(bytes_array))
}

/// Parses the object type discriminator bytes.
///
/// # Arguments
/// * `bytes` - The object type bytes (4 bytes)
///
/// # Returns
/// * `Ok(ObjectType)` - Successfully parsed object type
/// * `Err(ConsumerError)` - Invalid object type
fn parse_object_type(bytes: &[u8]) -> Result<ObjectType, ConsumerError> {
    if bytes.len() != 4 {
        return Err(ConversionError::InvalidObjectType(format!(
            "expected 4 bytes, got {}",
            bytes.len()
        ))
        .into());
    }

    // Interpret as big-endian u32 (Solidity bytes4 encoding)
    let type_id = u32::from_be_bytes(
        bytes
            .try_into()
            .map_err(|_| ConversionError::InvalidObjectType("failed to convert bytes".to_string()))?,
    );

    match type_id {
        0 => Ok(ObjectType::Entity),
        1 => Ok(ObjectType::Relation),
        _ => Err(ConversionError::InvalidObjectType(format!(
            "unknown type: {}",
            type_id
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;

    /// Creates ABI-encoded data for vote: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
    /// Layout:
    /// - bytes 0-31: uint16 version (right-aligned, value in bytes 30-31)
    /// - bytes 32-63: bytes16 groupId (left-aligned, value in bytes 32-47)
    /// - bytes 64-95: bytes16 spacePOV (left-aligned, value in bytes 64-79)
    fn create_abi_encoded_data(version: u16, group_id: &Uuid, space_pov: &Uuid) -> Vec<u8> {
        let mut data = vec![0u8; 96];

        // uint16 version at bytes 30-31 (big-endian)
        let version_bytes = version.to_be_bytes();
        data[30..32].copy_from_slice(&version_bytes);

        // bytes16 groupId at bytes 32-47
        data[32..48].copy_from_slice(group_id.as_bytes());

        // bytes16 spacePOV at bytes 64-79
        data[64..80].copy_from_slice(space_pov.as_bytes());

        data
    }

    fn create_test_vote(
        voter_id: [u8; 16],
        object_id: [u8; 16],
        object_type: u32,
        direction: VoteDirection,
        version: u16,
        group_id: &Uuid,
        space_pov: &Uuid,
    ) -> HermesVoteCast {
        HermesVoteCast {
            voter_id: voter_id.to_vec(),
            object_id: object_id.to_vec(),
            object_type: object_type.to_be_bytes().to_vec(), // big-endian to match Solidity bytes4
            direction: direction as i32,
            data: create_abi_encoded_data(version, group_id, space_pov),
            meta: Some(BlockchainMetadata {
                created_at: 1700000000,
                created_by: vec![],
                block_number: 12345,
                cursor: "cursor123".to_string(),
            }),
        }
    }

    #[test]
    fn test_hermes_vote_to_action_raw_upvote() {
        let voter_uuid = Uuid::new_v4();
        let object_uuid = Uuid::new_v4();
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = create_test_vote(
            *voter_uuid.as_bytes(),
            *object_uuid.as_bytes(),
            0, // Entity
            VoteDirection::Up,
            1, // version
            &group_uuid,
            &space_uuid,
        );

        let result = hermes_vote_to_action_raw(&vote).unwrap();

        assert_eq!(result.user_id, voter_uuid);
        assert_eq!(result.object_id, object_uuid);
        assert_eq!(result.object_type, ObjectType::Entity);
        assert_eq!(result.action_type, ActionType::Vote);
        assert_eq!(result.action_version, 1);
        assert_eq!(result.group_id, Some(group_uuid));
        assert_eq!(result.space_pov, space_uuid);
        assert_eq!(result.block_number, 12345);
        assert_eq!(result.block_timestamp, 1700000000);
        // Check vote direction is encoded in metadata
        assert_eq!(result.metadata.as_ref().unwrap()[0], 0); // Up
    }

    #[test]
    fn test_hermes_vote_to_action_raw_downvote() {
        let voter_uuid = Uuid::new_v4();
        let object_uuid = Uuid::new_v4();
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = create_test_vote(
            *voter_uuid.as_bytes(),
            *object_uuid.as_bytes(),
            1, // Relation
            VoteDirection::Down,
            2, // version
            &group_uuid,
            &space_uuid,
        );

        let result = hermes_vote_to_action_raw(&vote).unwrap();

        assert_eq!(result.object_type, ObjectType::Relation);
        assert_eq!(result.action_version, 2);
        assert_eq!(result.metadata.as_ref().unwrap()[0], 1); // Down
    }

    #[test]
    fn test_hermes_vote_to_action_raw_unvote() {
        let voter_uuid = Uuid::new_v4();
        let object_uuid = Uuid::new_v4();
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = create_test_vote(
            *voter_uuid.as_bytes(),
            *object_uuid.as_bytes(),
            0,
            VoteDirection::None,
            1,
            &group_uuid,
            &space_uuid,
        );

        let result = hermes_vote_to_action_raw(&vote).unwrap();

        assert_eq!(result.metadata.as_ref().unwrap()[0], 2); // Remove
    }

    #[test]
    fn test_hermes_vote_extracts_version_group_space() {
        let voter_uuid = Uuid::new_v4();
        let object_uuid = Uuid::new_v4();
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = create_test_vote(
            *voter_uuid.as_bytes(),
            *object_uuid.as_bytes(),
            0,
            VoteDirection::Up,
            42, // specific version to test
            &group_uuid,
            &space_uuid,
        );

        let result = hermes_vote_to_action_raw(&vote).unwrap();

        assert_eq!(result.action_version, 42);
        assert_eq!(result.group_id, Some(group_uuid));
        assert_eq!(result.space_pov, space_uuid);
    }

    #[test]
    fn test_invalid_voter_id_length() {
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = HermesVoteCast {
            voter_id: vec![0; 8], // Invalid: only 8 bytes
            object_id: vec![0; 16],
            object_type: vec![0; 4],
            direction: VoteDirection::Up as i32,
            data: create_abi_encoded_data(1, &group_uuid, &space_uuid),
            meta: Some(BlockchainMetadata {
                created_at: 0,
                created_by: vec![],
                block_number: 0,
                cursor: String::new(),
            }),
        };

        let result = hermes_vote_to_action_raw(&vote);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConsumerError::Conversion(ConversionError::InvalidUuid(_))
        ));
    }

    #[test]
    fn test_invalid_object_type() {
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = HermesVoteCast {
            voter_id: vec![0; 16],
            object_id: vec![0; 16],
            object_type: 99u32.to_be_bytes().to_vec(), // Invalid type (big-endian)
            direction: VoteDirection::Up as i32,
            data: create_abi_encoded_data(1, &group_uuid, &space_uuid),
            meta: Some(BlockchainMetadata {
                created_at: 0,
                created_by: vec![],
                block_number: 0,
                cursor: String::new(),
            }),
        };

        let result = hermes_vote_to_action_raw(&vote);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConsumerError::Conversion(ConversionError::InvalidObjectType(_))
        ));
    }

    #[test]
    fn test_missing_metadata() {
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();

        let vote = HermesVoteCast {
            voter_id: vec![0; 16],
            object_id: vec![0; 16],
            object_type: vec![0; 4],
            direction: VoteDirection::Up as i32,
            data: create_abi_encoded_data(1, &group_uuid, &space_uuid),
            meta: None, // Missing
        };

        let result = hermes_vote_to_action_raw(&vote);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConsumerError::Conversion(ConversionError::MissingField(_))
        ));
    }

    #[test]
    fn test_invalid_data_field_length() {
        let vote = HermesVoteCast {
            voter_id: vec![0; 16],
            object_id: vec![0; 16],
            object_type: vec![0; 4],
            direction: VoteDirection::Up as i32,
            data: vec![0; 50], // Wrong size (should be 96)
            meta: Some(BlockchainMetadata {
                created_at: 0,
                created_by: vec![],
                block_number: 0,
                cursor: String::new(),
            }),
        };

        let result = hermes_vote_to_action_raw(&vote);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConsumerError::Conversion(ConversionError::InvalidDataField(_))
        ));
    }

    #[test]
    fn test_bytes_to_uuid() {
        let uuid = Uuid::new_v4();
        let result = bytes_to_uuid(uuid.as_bytes(), "test").unwrap();
        assert_eq!(result, uuid);
    }

    #[test]
    fn test_bytes_to_uuid_invalid_length() {
        let result = bytes_to_uuid(&[0; 8], "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_vote_data() {
        let group_uuid = Uuid::new_v4();
        let space_uuid = Uuid::new_v4();
        let data = create_abi_encoded_data(123, &group_uuid, &space_uuid);

        let (version, group_id, space_pov) = parse_vote_data(&data).unwrap();

        assert_eq!(version, 123);
        assert_eq!(group_id, group_uuid);
        assert_eq!(space_pov, space_uuid);
    }

    #[test]
    fn test_parse_vote_data_invalid_length() {
        let result = parse_vote_data(&[0; 50]);
        assert!(result.is_err());
    }
}
