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
//! - `SUBSPACE_VERIFIED`: Verified trust extension (explicit canonical trust)
//! - `SUBSPACE_RELATED`: Related trust extension (explicit non-canonical trust)
//! - `SUBSPACE_TOPIC_DECLARED`: Topic-based trust extension

use crate::events::{
    BlockMetadata, SpaceCreated, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TrustExtended, TrustExtension,
};
use hermes_relay::{actions, Action};

/// Convert a slice to a fixed-size array, returning None if length doesn't match.
fn to_array<const N: usize>(slice: &[u8]) -> Option<[u8; N]> {
    slice.try_into().ok()
}

/// Convert an Action to a SpaceTopologyEvent, if it's a topology-relevant action.
///
/// Returns `Some(event)` for:
/// - `SPACE_REGISTERED` actions → SpaceCreated
/// - `SUBSPACE_VERIFIED` actions → TrustExtended (Verified)
/// - `SUBSPACE_RELATED` actions → TrustExtended (Related)
/// - `SUBSPACE_TOPIC_DECLARED` actions → TrustExtended (Subtopic)
///
/// Returns `None` for other action types (edits, proposals, etc.)
pub fn convert_action(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let action_type = action.action.as_slice();

    if actions::matches(action_type, &actions::SPACE_REGISTERED) {
        convert_space_registered(action, meta)
    } else if actions::matches(action_type, &actions::SUBSPACE_VERIFIED) {
        convert_subspace_verified(action, meta)
    } else if actions::matches(action_type, &actions::SUBSPACE_RELATED) {
        convert_subspace_related(action, meta)
    } else if actions::matches(action_type, &actions::SUBSPACE_TOPIC_DECLARED) {
        convert_subspace_topic_declared(action, meta)
    } else {
        None
    }
}

/// Convert a SPACE_REGISTERED action to SpaceCreated event.
///
/// New action format (Space Registry v2):
/// - `from_id`: zeros (16 bytes)
/// - `to_id`: space_id (16 bytes)
/// - `topic`: registrar address as bytes32(bytes20(address))
///   - For EOA spaces: the owner's address
///   - For DAO spaces: the DAOSpace contract address
/// - `data`: empty (space type comes from separate SPACE_TYPE_DECLARED event)
///
/// Note: Space type (Personal vs DAO) is determined by a separate SPACE_TYPE_DECLARED
/// event, not from this event. We default to Personal with the registrar as owner.
fn convert_space_registered(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    // Space ID is now in to_id (from_id is zeros in new format)
    let space_id = to_array::<16>(&action.to_id)?;

    // Registrar address is in topic - this is the owner for personal spaces
    let owner = to_array::<32>(&action.topic)?;
    let space_type = SpaceType::Personal { owner };

    // Topic ID defaults to zeros - spaces can declare topics separately
    let topic_id = [0u8; 16];

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id,
            topic_id,
            space_type,
        }),
    })
}

/// Convert a SUBSPACE_VERIFIED action to TrustExtended event.
///
/// Action format:
/// - `from_id`: source_space_id (16 bytes)
/// - `topic[0..16]`: padding (zeros)
/// - `topic[16..32]`: target_space_id (16 bytes)
fn convert_subspace_verified(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let source_space_id = to_array::<16>(&action.from_id)?;

    if action.topic.len() < 32 {
        return None;
    }
    let target_space_id = to_array::<16>(&action.topic[16..32])?;

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id,
            extension: TrustExtension::Verified { target_space_id },
        }),
    })
}

/// Convert a SUBSPACE_RELATED action to TrustExtended event.
///
/// Action format:
/// - `from_id`: source_space_id (16 bytes)
/// - `topic[0..16]`: padding (zeros)
/// - `topic[16..32]`: target_space_id (16 bytes)
fn convert_subspace_related(action: &Action, meta: &BlockMetadata) -> Option<SpaceTopologyEvent> {
    let source_space_id = to_array::<16>(&action.from_id)?;

    if action.topic.len() < 32 {
        return None;
    }
    let target_space_id = to_array::<16>(&action.topic[16..32])?;

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id,
            extension: TrustExtension::Related { target_space_id },
        }),
    })
}

/// Convert a SUBSPACE_TOPIC_DECLARED action to TrustExtended event.
///
/// Action format:
/// - `from_id`: source_space_id (16 bytes)
/// - `topic[0..16]`: subspace_id (16 bytes)
/// - `topic[16..32]`: topic_id (16 bytes)
fn convert_subspace_topic_declared(
    action: &Action,
    meta: &BlockMetadata,
) -> Option<SpaceTopologyEvent> {
    let source_space_id = to_array::<16>(&action.from_id)?;

    if action.topic.len() < 32 {
        return None;
    }
    // Topic ID is in the last 16 bytes
    let target_topic_id = to_array::<16>(&action.topic[16..32])?;

    Some(SpaceTopologyEvent {
        meta: meta.clone(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id,
            extension: TrustExtension::Subtopic { target_topic_id },
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_relay::source::mock_events::{
        self, make_address, make_id, space_id_registered, subspace_related,
        subspace_topic_declared, subspace_verified,
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
    fn test_convert_space_id_registered() {
        // New format: space_id in to_id, registrar in topic
        let action = space_id_registered(make_id(0x01), make_address(0xAA));
        let meta = test_meta();

        let event = convert_action(&action, &meta).expect("should convert");

        assert_eq!(event.meta.block_number, 100);
        match event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(created.space_id, make_id(0x01));
                // All SPACE_ID_REGISTERED events are treated as Personal
                // (DAO detection requires separate SPACE_TYPE_DECLARED event)
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
    fn test_convert_subspace_verified() {
        let action = subspace_verified(make_id(0x01), make_id(0x02));
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
    fn test_convert_subspace_related() {
        let action = subspace_related(make_id(0x01), make_id(0x02));
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
    fn test_convert_subspace_topic_declared() {
        let action = subspace_topic_declared(make_id(0x01), make_id(0x02), make_id(0x8A));
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
        // 10 verified + 4 related + 5 topic declarations = 19 trust extensions
        assert_eq!(trust_count, 19);
        // Total topology events (edits, proposals, flagging, voting filtered out)
        assert_eq!(events.len(), 37);
    }
}
