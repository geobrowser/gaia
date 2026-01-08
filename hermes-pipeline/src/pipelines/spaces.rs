//! Pipeline: SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED → space.creations
//!
//! Squashes paired registration events into HermesCreateSpace events.
//!
//! The contract emits two events for each space registration:
//! 1. SPACE_ID_REGISTERED: Contains space_id (in to_id) and owner address (in topic)
//! 2. SPACE_TYPE_DECLARED: Contains space_type (in topic) indicating DAO or EOA
//!
//! These are squashed into a single HermesCreateSpace with the appropriate payload type.

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::space::{
    DefaultDaoSpacePayload, EoaSpacePayload, HermesCreateSpace, hermes_create_space,
};

use super::BlockMetadata;

/// Result of transforming space actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed space creation events ready for emission.
    pub events: Vec<HermesCreateSpace>,
}

/// Transform all space registration actions in a block.
///
/// Squashes SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED pairs into single events.
/// For each SPACE_ID_REGISTERED, looks ahead for a matching SPACE_TYPE_DECLARED
/// with the same space_id to determine the space type.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut events = Vec::new();

    for (index, action) in actions.iter().enumerate() {
        if !actions::matches(&action.action, &actions::SPACE_REGISTERED) {
            continue;
        }

        // Extract space_id from to_id (new contract structure)
        let space_id = &action.to_id;

        // Look ahead for paired SPACE_TYPE_DECLARED with same space_id
        let space_type = find_space_type_declared(actions, index + 1, space_id);

        let sequence = index as u32;
        let event = debug_span!("convert.space", space_id = %hex::encode(space_id))
            .in_scope(|| convert(action, space_type.as_deref(), meta, sequence))?;
        events.push(event);
    }

    Ok(TransformResult { events })
}

/// Find a SPACE_TYPE_DECLARED action for the given space_id, starting from start_index.
fn find_space_type_declared(
    actions: &[Action],
    start_index: usize,
    space_id: &[u8],
) -> Option<Vec<u8>> {
    for action in actions.iter().skip(start_index) {
        if actions::matches(&action.action, &actions::SPACE_TYPE_DECLARED)
            && action.from_id == space_id
        {
            return Some(action.topic.clone());
        }
    }
    None
}

/// Convert a SPACE_ID_REGISTERED action to HermesCreateSpace proto.
///
/// The action structure for SPACE_ID_REGISTERED:
/// - from_id: bytes16(0) (zeros)
/// - to_id: space_id (16 bytes)
/// - topic: bytes32(bytes20(owner_address))
/// - data: empty
///
/// The space_type comes from the paired SPACE_TYPE_DECLARED event's topic field.
fn convert(
    action: &Action,
    space_type: Option<&[u8]>,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesCreateSpace> {
    let space_id = action.to_id.clone();
    // Extract 20-byte address from 32-byte topic (address is left-aligned, right-padded with zeros)
    let owner_address = action
        .topic
        .get(0..20)
        .map(|s| s.to_vec())
        .unwrap_or_default();

    // Determine payload based on space_type
    let payload = match space_type {
        Some(st) if st == actions::SPACE_TYPE_DAO => {
            // DAO space - editors/members come from separate events
            Some(hermes_create_space::Payload::DefaultDaoSpace(
                DefaultDaoSpacePayload {
                    address: owner_address.clone(),
                },
            ))
        }
        Some(st) if st == actions::SPACE_TYPE_EOA => {
            // EOA space with owner
            Some(hermes_create_space::Payload::EoaSpace(EoaSpacePayload {
                owner: owner_address.clone(),
            }))
        }
        Some(st) => {
            // Unknown space type - log warning and default to EOA
            warn!(
                space_type = %hex::encode(st),
                space_id = %hex::encode(&space_id),
                "Unknown space type, defaulting to EOA"
            );
            Some(hermes_create_space::Payload::EoaSpace(EoaSpacePayload {
                owner: owner_address.clone(),
            }))
        }
        None => {
            // No SPACE_TYPE_DECLARED found - could be legacy or error
            // Default to EOA space
            warn!(
                space_id = %hex::encode(&space_id),
                "No SPACE_TYPE_DECLARED found, defaulting to EOA"
            );
            Some(hermes_create_space::Payload::EoaSpace(EoaSpacePayload {
                owner: owner_address.clone(),
            }))
        }
    };

    Ok(HermesCreateSpace {
        space_id,
        payload,
        meta: Some(meta.to_proto(sequence)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    #[test]
    fn test_convert_eoa_space_with_type() {
        // Create a 32-byte topic with distinct address bytes (first 20) and padding (last 12)
        let mut topic: Vec<u8> = (1..=20).collect();
        topic.resize(32, 0); // Pad to 32 bytes with zeros

        let action = Action {
            from_id: vec![0; 16], // zeros for SPACE_ID_REGISTERED
            to_id: vec![1; 16],   // space_id
            action: actions::SPACE_REGISTERED.to_vec(),
            topic,
            data: vec![],
        };

        let result = convert(&action, Some(&actions::SPACE_TYPE_EOA), &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);

        // Verify owner is extracted as 20 bytes, not full 32 bytes
        if let Some(hermes_create_space::Payload::EoaSpace(eoa)) = &result.payload {
            assert_eq!(eoa.owner.len(), 20, "Owner should be 20 bytes, not 32");
            let expected_owner: Vec<u8> = (1..=20).collect();
            assert_eq!(eoa.owner, expected_owner);
        } else {
            panic!("Expected EoaSpace payload");
        }
    }

    #[test]
    fn test_convert_dao_space_with_type() {
        let action = Action {
            from_id: vec![0; 16], // zeros for SPACE_ID_REGISTERED
            to_id: vec![1; 16],   // space_id
            action: actions::SPACE_REGISTERED.to_vec(),
            topic: vec![2; 32], // owner address
            data: vec![],
        };

        let result = convert(&action, Some(&actions::SPACE_TYPE_DAO), &test_meta(), 0).unwrap();
        assert!(matches!(
            result.payload,
            Some(hermes_create_space::Payload::DefaultDaoSpace(_))
        ));
    }

    #[test]
    fn test_convert_without_type_defaults_to_eoa() {
        let action = Action {
            from_id: vec![0; 16],
            to_id: vec![1; 16],
            action: actions::SPACE_REGISTERED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert(&action, None, &test_meta(), 0).unwrap();
        assert!(matches!(
            result.payload,
            Some(hermes_create_space::Payload::EoaSpace(_))
        ));
    }

    #[test]
    fn test_transform_squashes_registered_and_type_declared() {
        let space_id = vec![1; 16];

        let test_actions = vec![
            // SPACE_ID_REGISTERED
            Action {
                from_id: vec![0; 16],
                to_id: space_id.clone(),
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![2; 32], // owner address
                data: vec![],
            },
            // SPACE_TYPE_DECLARED (paired with above)
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::SPACE_TYPE_DECLARED.to_vec(),
                topic: actions::SPACE_TYPE_DAO.to_vec(),
                data: vec![],
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 1);
        assert!(matches!(
            result.events[0].payload,
            Some(hermes_create_space::Payload::DefaultDaoSpace(_))
        ));
    }

    #[test]
    fn test_transform_filters_non_space_registered() {
        let test_actions = vec![
            Action {
                from_id: vec![0; 16],
                to_id: vec![1; 16],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![2; 16],
                to_id: vec![3; 16],
                action: actions::SUBSPACE_ADDED.to_vec(), // Different action type
                topic: vec![3; 32],
                data: vec![],
            },
            Action {
                from_id: vec![0; 16],
                to_id: vec![4; 16],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![5; 32],
                data: vec![],
            },
        ];

        let result = transform(&test_actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 2); // Only SPACE_REGISTERED actions
    }

    #[test]
    fn test_find_space_type_declared() {
        let space_id = vec![1; 16];

        let test_actions = vec![
            Action {
                from_id: vec![0; 16],
                to_id: space_id.clone(),
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: space_id.clone(),
                to_id: space_id.clone(),
                action: actions::SPACE_TYPE_DECLARED.to_vec(),
                topic: actions::SPACE_TYPE_EOA.to_vec(),
                data: vec![],
            },
        ];

        let result = find_space_type_declared(&test_actions, 1, &space_id);
        assert_eq!(result, Some(actions::SPACE_TYPE_EOA.to_vec()));
    }

    #[test]
    fn test_find_space_type_declared_not_found() {
        let space_id = vec![1; 16];
        let other_space_id = vec![2; 16];

        let test_actions = vec![
            Action {
                from_id: vec![0; 16],
                to_id: space_id.clone(),
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            // SPACE_TYPE_DECLARED for different space
            Action {
                from_id: other_space_id.clone(),
                to_id: other_space_id.clone(),
                action: actions::SPACE_TYPE_DECLARED.to_vec(),
                topic: actions::SPACE_TYPE_EOA.to_vec(),
                data: vec![],
            },
        ];

        let result = find_space_type_declared(&test_actions, 1, &space_id);
        assert_eq!(result, None);
    }
}
