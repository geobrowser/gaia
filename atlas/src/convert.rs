//! Conversion from hermes-relay Action types to Atlas internal event types.
//!
//! This module provides functions to convert `Action` events from the
//! hermes-relay crate into Atlas's internal event types used by the graph
//! processing pipeline.
//!
//! ## Action Format
//!
//! Actions from hermes-substream have the following structure:
//! - `from_id`: 16 bytes - source space ID
//! - `to_id`: 16 bytes - target space ID (often unused)
//! - `action`: 32 bytes - keccak256 hash identifying the action type
//! - `topic`: 32 bytes - context-dependent data
//! - `data`: variable - action-specific payload
//!
//! ## Relevant Action Types for Atlas
//!
//! Atlas processes topology events:
//! - `SPACE_REGISTERED`: New space creation
//! - `SUBSPACE_ADDED`: Trust extension between spaces

use crate::events::{
    BlockMetadata, SpaceCreated, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TrustExtended, TrustExtension,
};
use hermes_relay::{actions, Action};

// Trust extension type bytes (first 2 bytes of data field)
const TRUST_TYPE_VERIFIED: [u8; 2] = [0x00, 0x00];
const TRUST_TYPE_RELATED: [u8; 2] = [0x00, 0x01];
const TRUST_TYPE_SUBTOPIC: [u8; 2] = [0x00, 0x02];

/// Convert a slice to a fixed-size array, returning None if length doesn't match.
fn to_array<const N: usize>(slice: &[u8]) -> Option<[u8; N]> {
    slice.try_into().ok()
}

/// Convert an Action to a SpaceTopologyEvent, if it's a topology-relevant action.
///
/// Returns `Some(event)` for:
/// - `SPACE_REGISTERED` actions → SpaceCreated
/// - `SUBSPACE_ADDED` actions → TrustExtended
///
/// Returns `None` for other action types (edits, proposals, etc.)
pub fn convert_action(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let action_type = action.action.as_slice();

    if actions::matches(action_type, &actions::SPACE_REGISTERED) {
        convert_space_registered(action, meta)
    } else if actions::matches(action_type, &actions::SUBSPACE_ADDED) {
        convert_subspace_added(action, meta)
    } else {
        None
    }
}

/// Convert a SPACE_REGISTERED action to SpaceCreated event.
///
/// Action format:
/// - `from_id`: space_id (16 bytes)
/// - `topic`: owner address (32 bytes) for personal spaces
/// - `data`: DAO membership data (if DAO space)
fn convert_space_registered(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let space_id = to_array::<16>(&action.from_id)?;

    // For personal spaces, topic contains the owner address
    // For DAO spaces, topic is zeroed and data contains editor/member lists
    let space_type = if action.data.is_empty() {
        // Personal space - topic is owner address
        let owner = to_array::<32>(&action.topic)?;
        SpaceType::Personal { owner }
    } else {
        // DAO space - parse editor/member lists from data
        let (initial_editors, initial_members) = parse_dao_data(&action.data)?;
        SpaceType::Dao {
            initial_editors,
            initial_members,
        }
    };

    // For space creation, the topic_id is derived from the space_id
    // In the mock implementation, spaces announce a topic at creation
    // We use the first 16 bytes of the topic field as topic_id
    let topic_id = to_array::<16>(&action.topic[..16.min(action.topic.len())]).unwrap_or([0u8; 16]);

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id,
            topic_id,
            space_type,
        }),
    })
}

/// Parse DAO data field to extract initial editors and members.
///
/// Format:
/// - 2 bytes: number of editors (big-endian u16)
/// - N * 16 bytes: editor space IDs
/// - 2 bytes: number of members (big-endian u16)
/// - M * 16 bytes: member space IDs
#[allow(clippy::type_complexity)]
fn parse_dao_data(data: &[u8]) -> Option<(Vec<[u8; 16]>, Vec<[u8; 16]>)> {
    if data.len() < 4 {
        return Some((vec![], vec![]));
    }

    let mut offset = 0;

    // Read number of editors
    let num_editors = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;

    // Read editor IDs
    let mut editors = Vec::with_capacity(num_editors);
    for _ in 0..num_editors {
        if offset + 16 > data.len() {
            break;
        }
        if let Some(id) = to_array::<16>(&data[offset..offset + 16]) {
            editors.push(id);
        }
        offset += 16;
    }

    // Read number of members
    if offset + 2 > data.len() {
        return Some((editors, vec![]));
    }
    let num_members = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;

    // Read member IDs
    let mut members = Vec::with_capacity(num_members);
    for _ in 0..num_members {
        if offset + 16 > data.len() {
            break;
        }
        if let Some(id) = to_array::<16>(&data[offset..offset + 16]) {
            members.push(id);
        }
        offset += 16;
    }

    Some((editors, members))
}

/// Convert a SUBSPACE_ADDED action to TrustExtended event.
///
/// Action format:
/// - `from_id`: source_space_id (16 bytes)
/// - `topic[0..16]`: padding (zeros)
/// - `topic[16..32]`: target_space_id or target_topic_id (16 bytes)
/// - `data[0..2]`: trust type (VERIFIED=0x0000, RELATED=0x0001, SUBTOPIC=0x0002)
fn convert_subspace_added(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let source_space_id = to_array::<16>(&action.from_id)?;

    // Extract target from topic[16..32]
    if action.topic.len() < 32 {
        return None;
    }
    let target_id = to_array::<16>(&action.topic[16..32])?;

    // Parse trust type from data field
    let extension = if action.data.len() >= 2 {
        let trust_type: [u8; 2] = [action.data[0], action.data[1]];
        match trust_type {
            TRUST_TYPE_VERIFIED => TrustExtension::Verified {
                target_space_id: target_id,
            },
            TRUST_TYPE_RELATED => TrustExtension::Related {
                target_space_id: target_id,
            },
            TRUST_TYPE_SUBTOPIC => TrustExtension::Subtopic {
                target_topic_id: target_id,
            },
            _ => TrustExtension::Verified {
                target_space_id: target_id,
            },
        }
    } else {
        // Default to verified if no type specified
        TrustExtension::Verified {
            target_space_id: target_id,
        }
    };

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id,
            extension,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_relay::source::mock_events::{
        self, make_address, make_id, space_created, space_created_dao, trust_extended_related,
        trust_extended_subtopic, trust_extended_verified,
    };

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            block_number: 100,
            block_timestamp: 1200,
            tx_hash: "0xabc".to_string(),
            cursor: "cursor_1".to_string(),
        }
    }

    #[test]
    fn test_convert_space_created_personal() {
        let action = space_created(make_id(0x01), make_address(0xAA));
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        assert_eq!(event.meta.block_number, 100);
        match event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(created.space_id, make_id(0x01));
                match created.space_type {
                    SpaceType::Personal { owner } => {
                        assert_eq!(owner, make_address(0xAA));
                    }
                    _ => panic!("Expected Personal space type"),
                }
            }
            _ => panic!("Expected SpaceCreated"),
        }
    }

    #[test]
    fn test_convert_space_created_dao() {
        let action = space_created_dao(make_id(0x10), vec![make_id(0x11)], vec![make_id(0x12)]);
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        match event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(created.space_id, make_id(0x10));
                match created.space_type {
                    SpaceType::Dao {
                        initial_editors,
                        initial_members,
                    } => {
                        assert_eq!(initial_editors.len(), 1);
                        assert_eq!(initial_editors[0], make_id(0x11));
                        assert_eq!(initial_members.len(), 1);
                        assert_eq!(initial_members[0], make_id(0x12));
                    }
                    _ => panic!("Expected Dao space type"),
                }
            }
            _ => panic!("Expected SpaceCreated"),
        }
    }

    #[test]
    fn test_convert_trust_extended_verified() {
        let action = trust_extended_verified(make_id(0x01), make_id(0x02));
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => {
                assert_eq!(extended.source_space_id, make_id(0x01));
                match extended.extension {
                    TrustExtension::Verified { target_space_id } => {
                        assert_eq!(target_space_id, make_id(0x02));
                    }
                    _ => panic!("Expected Verified extension"),
                }
            }
            _ => panic!("Expected TrustExtended"),
        }
    }

    #[test]
    fn test_convert_trust_extended_related() {
        let action = trust_extended_related(make_id(0x01), make_id(0x02));
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => match extended.extension {
                TrustExtension::Related { target_space_id } => {
                    assert_eq!(target_space_id, make_id(0x02));
                }
                _ => panic!("Expected Related extension"),
            },
            _ => panic!("Expected TrustExtended"),
        }
    }

    #[test]
    fn test_convert_trust_extended_subtopic() {
        let action = trust_extended_subtopic(make_id(0x01), make_id(0x8A));
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => match extended.extension {
                TrustExtension::Subtopic { target_topic_id } => {
                    assert_eq!(target_topic_id, make_id(0x8A));
                }
                _ => panic!("Expected Subtopic extension"),
            },
            _ => panic!("Expected TrustExtended"),
        }
    }

    #[test]
    fn test_convert_edit_published_returns_none() {
        let action = mock_events::edit_published(make_id(0x01), "QmTestHash");
        let meta = test_meta();

        let event = convert_action(&action, &meta);
        assert!(event.is_none(), "Edit events should be filtered out");
    }

    #[test]
    fn test_topology_generate_counts() {
        let actions = mock_events::test_topology::generate();
        let meta = test_meta();

        let events: Vec<_> = actions
            .iter()
            .filter_map(|a| convert_action(a, &meta))
            .collect();

        let space_count = events
            .iter()
            .filter(|e| matches!(e.payload, SpaceTopologyPayload::SpaceCreated(_)))
            .count();
        let trust_count = events
            .iter()
            .filter(|e| matches!(e.payload, SpaceTopologyPayload::TrustExtended(_)))
            .count();

        // 18 spaces: 11 canonical + 7 non-canonical
        assert_eq!(space_count, 18);
        // 14 explicit edges + 5 topic edges = 19 trust extensions
        assert_eq!(trust_count, 19);
        // Total topology events (edits filtered out)
        assert_eq!(events.len(), 37);
    }
}
