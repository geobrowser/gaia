//! Conversion functions from raw Actions to Hermes proto types.
//!
//! These functions take raw Action messages from hermes-substream and convert
//! them into typed Hermes protobuf messages for downstream processing.

use anyhow::Result;

use hermes_relay::stream::utils::BlockMetadata;
use hermes_relay::Action;
use hermes_schema::pb::{
    blockchain_metadata::BlockchainMetadata,
    space::{
        hermes_create_space, hermes_space_trust_extension, DefaultDaoSpacePayload,
        HermesCreateSpace, HermesSpaceTrustExtension, PersonalSpacePayload, VerifiedExtension,
    },
};

/// Convert block metadata to BlockchainMetadata proto
fn convert_block_metadata(meta: &BlockMetadata) -> BlockchainMetadata {
    // Parse the timestamp string to u64
    let created_at: u64 = meta.timestamp.parse().unwrap_or(0);

    BlockchainMetadata {
        created_at,
        created_by: vec![], // Not available in block metadata
        block_number: meta.block_number,
        cursor: meta.cursor.clone(),
    }
}

/// Convert a SPACE_REGISTERED action to HermesCreateSpace proto.
///
/// The action structure for SPACE_REGISTERED:
/// - from_id: space_id (16 bytes)
/// - to_id: space_id (16 bytes, same as from_id)
/// - topic: space_address (20 bytes, padded to 32)
/// - data: encoded space creation payload
pub fn convert_space_registered(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesCreateSpace> {
    let space_id = action.from_id.clone();

    // For now, we'll create a basic space without decoding the payload
    // The payload contains space type information that we can decode later
    // when we have the full ABI/encoding specification

    // Default to a DAO space since that's the most common case
    // In a full implementation, we'd decode the data field to determine the space type
    let payload = if action.data.is_empty() {
        // Personal space (no extra data)
        Some(hermes_create_space::Payload::PersonalSpace(
            PersonalSpacePayload {
                owner: action.topic.clone(), // The topic contains the space address
            },
        ))
    } else {
        // DAO space with encoded members/editors
        // For now, use empty lists - full decoding would parse the data field
        Some(hermes_create_space::Payload::DefaultDaoSpace(
            DefaultDaoSpacePayload {
                initial_editors: vec![],
                initial_members: vec![],
            },
        ))
    };

    Ok(HermesCreateSpace {
        space_id,
        topic_id: action.topic.clone(),
        payload,
        meta: Some(convert_block_metadata(meta)),
    })
}

/// Convert a SUBSPACE_ADDED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_ADDED:
/// - from_id: parent_space_id (16 bytes)
/// - to_id: subspace_id (16 bytes)
/// - topic: subspace_id padded to 32 bytes
/// - data: encoded trust type and metadata
pub fn convert_subspace_added(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.to_id.clone();

    // Determine the extension type from the data field
    // For now, default to Verified - in a full implementation we'd decode the data
    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(convert_block_metadata(meta)),
    })
}

/// Convert a SUBSPACE_REMOVED action to HermesSpaceTrustExtension proto.
///
/// Uses the same structure as SUBSPACE_ADDED but represents a trust revocation.
/// Downstream consumers can detect removal by comparing with previous state
/// or by using a separate topic/header.
pub fn convert_subspace_removed(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.to_id.clone();

    // Same structure as added - the removal semantics are handled at the Kafka level
    // (e.g., using a header to indicate removal, or sending to a different topic)
    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(convert_block_metadata(meta)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_block_metadata() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    #[test]
    fn test_convert_space_registered() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![1; 16],
            action: vec![0; 32],
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_space_registered(&action, &test_block_metadata()).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert!(result.payload.is_some());
        assert!(result.meta.is_some());
    }

    #[test]
    fn test_convert_subspace_added() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: vec![0; 32],
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_subspace_added(&action, &test_block_metadata()).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        assert!(result.extension.is_some());
        assert!(result.meta.is_some());
    }
}
