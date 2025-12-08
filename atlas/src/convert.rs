//! Conversion from mock_substream crate types to Atlas internal event types.
//!
//! This module provides `From` implementations to convert events from the
//! shared `mock_substream` crate into Atlas's internal event types used
//! by the graph processing pipeline.

use crate::events::{
    BlockMetadata, SpaceCreated, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TrustExtended, TrustExtension,
};

/// Convert mock_substream BlockMetadata to Atlas BlockMetadata
impl From<&mock_substream::BlockMetadata> for BlockMetadata {
    fn from(meta: &mock_substream::BlockMetadata) -> Self {
        BlockMetadata {
            block_number: meta.block_number,
            block_timestamp: meta.block_timestamp,
            tx_hash: meta.tx_hash.clone(),
            cursor: meta.cursor.clone(),
        }
    }
}

/// Convert mock_substream SpaceType to Atlas SpaceType
impl From<&mock_substream::SpaceType> for SpaceType {
    fn from(space_type: &mock_substream::SpaceType) -> Self {
        match space_type {
            mock_substream::SpaceType::Personal { owner } => SpaceType::Personal { owner: *owner },
            mock_substream::SpaceType::Dao {
                initial_editors,
                initial_members,
            } => SpaceType::Dao {
                initial_editors: initial_editors.clone(),
                initial_members: initial_members.clone(),
            },
        }
    }
}

/// Convert mock_substream TrustExtension to Atlas TrustExtension
impl From<&mock_substream::TrustExtension> for TrustExtension {
    fn from(extension: &mock_substream::TrustExtension) -> Self {
        match extension {
            mock_substream::TrustExtension::Verified { target_space_id } => {
                TrustExtension::Verified {
                    target_space_id: *target_space_id,
                }
            }
            mock_substream::TrustExtension::Related { target_space_id } => {
                TrustExtension::Related {
                    target_space_id: *target_space_id,
                }
            }
            mock_substream::TrustExtension::Subtopic { target_topic_id } => {
                TrustExtension::Subtopic {
                    target_topic_id: *target_topic_id,
                }
            }
        }
    }
}

/// Convert mock_substream SpaceCreated to Atlas SpaceTopologyEvent
impl From<&mock_substream::SpaceCreated> for SpaceTopologyEvent {
    fn from(event: &mock_substream::SpaceCreated) -> Self {
        SpaceTopologyEvent {
            meta: BlockMetadata::from(&event.meta),
            payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
                space_id: event.space_id,
                topic_id: event.topic_id,
                space_type: SpaceType::from(&event.space_type),
            }),
        }
    }
}

/// Convert mock_substream TrustExtended to Atlas SpaceTopologyEvent
impl From<&mock_substream::TrustExtended> for SpaceTopologyEvent {
    fn from(event: &mock_substream::TrustExtended) -> Self {
        SpaceTopologyEvent {
            meta: BlockMetadata::from(&event.meta),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: event.source_space_id,
                extension: TrustExtension::from(&event.extension),
            }),
        }
    }
}

/// Convert a MockEvent to an optional SpaceTopologyEvent.
///
/// Returns `Some(event)` for SpaceCreated and TrustExtended events.
/// Returns `None` for EditPublished events (Atlas only processes topology).
pub fn convert_mock_event(event: &mock_substream::MockEvent) -> Option<SpaceTopologyEvent> {
    match event {
        mock_substream::MockEvent::SpaceCreated(space) => Some(SpaceTopologyEvent::from(space)),
        mock_substream::MockEvent::TrustExtended(trust) => Some(SpaceTopologyEvent::from(trust)),
        mock_substream::MockEvent::EditPublished(_) => None, // Atlas ignores edits
    }
}

/// Convert a list of MockBlocks to SpaceTopologyEvents.
///
/// Filters out EditPublished events and flattens blocks into a single event stream.
pub fn convert_mock_blocks(blocks: &[mock_substream::MockBlock]) -> Vec<SpaceTopologyEvent> {
    blocks
        .iter()
        .flat_map(|block| &block.events)
        .filter_map(convert_mock_event)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_substream::test_topology;

    #[test]
    fn test_convert_space_created() {
        let mock_event = mock_substream::SpaceCreated {
            meta: mock_substream::BlockMetadata {
                block_number: 100,
                block_timestamp: 1200,
                tx_hash: "0xabc".to_string(),
                cursor: "cursor_1".to_string(),
            },
            space_id: mock_substream::make_id(0x01),
            topic_id: mock_substream::make_id(0x02),
            space_type: mock_substream::SpaceType::Personal {
                owner: mock_substream::make_address(0xAA),
            },
        };

        let atlas_event = SpaceTopologyEvent::from(&mock_event);

        assert_eq!(atlas_event.meta.block_number, 100);
        match atlas_event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(created.space_id, mock_substream::make_id(0x01));
                assert_eq!(created.topic_id, mock_substream::make_id(0x02));
            }
            _ => panic!("Expected SpaceCreated"),
        }
    }

    #[test]
    fn test_convert_trust_extended() {
        let mock_event = mock_substream::TrustExtended {
            meta: mock_substream::BlockMetadata {
                block_number: 200,
                block_timestamp: 2400,
                tx_hash: "0xdef".to_string(),
                cursor: "cursor_2".to_string(),
            },
            source_space_id: mock_substream::make_id(0x01),
            extension: mock_substream::TrustExtension::Verified {
                target_space_id: mock_substream::make_id(0x02),
            },
        };

        let atlas_event = SpaceTopologyEvent::from(&mock_event);

        match atlas_event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => {
                assert_eq!(extended.source_space_id, mock_substream::make_id(0x01));
                match extended.extension {
                    TrustExtension::Verified { target_space_id } => {
                        assert_eq!(target_space_id, mock_substream::make_id(0x02));
                    }
                    _ => panic!("Expected Verified extension"),
                }
            }
            _ => panic!("Expected TrustExtended"),
        }
    }

    #[test]
    fn test_convert_mock_blocks_filters_edits() {
        let blocks = test_topology::generate();
        let events = convert_mock_blocks(&blocks);

        // Should have 18 spaces + 19 trust extensions = 37 topology events
        // (6 edits are filtered out)
        assert_eq!(events.len(), 37);

        // Verify no edit events came through
        for event in &events {
            match &event.payload {
                SpaceTopologyPayload::SpaceCreated(_) => {}
                SpaceTopologyPayload::TrustExtended(_) => {}
            }
        }
    }
}
