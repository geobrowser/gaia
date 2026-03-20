//! Shared test helpers for Atlas unit tests.
//!
//! Provides factory functions for constructing test data (IDs, metadata,
//! events, graph state) so each test module doesn't duplicate them.

use crate::events::{
    BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, SpaceType,
    TopicId, TrustExtended, TrustExtension,
};
use crate::graph::GraphState;

/// Create a SpaceId with the given byte in the last position (all others zero).
pub fn make_space_id(n: u8) -> SpaceId {
    let mut id = [0u8; 16];
    id[15] = n;
    id
}

/// Create a TopicId with the given byte in the last position (all others zero).
pub fn make_topic_id(n: u8) -> TopicId {
    let mut id = [0u8; 16];
    id[15] = n;
    id
}

/// Create block metadata with fixed values suitable for tests.
pub fn make_block_meta() -> BlockMetadata {
    make_block_meta_at(1)
}

/// Create block metadata for a specific block number.
///
/// Useful when tests need events at distinct blocks (e.g., ordering by block).
pub fn make_block_meta_at(block: u64) -> BlockMetadata {
    BlockMetadata {
        block_number: block,
        block_timestamp: block * 12,
        tx_hash: format!("0x{:064x}", block),
        cursor: format!("cursor_{}", block),
    }
}

/// Create a space in the graph state and return its SpaceId.
/// The space's topic is `make_topic_id(n)`.
pub fn create_space(state: &mut GraphState, n: u8) -> SpaceId {
    let space = make_space_id(n);
    let topic = make_topic_id(n);
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id: space,
            topic_id: topic,
            space_type: SpaceType::Dao {
                initial_editors: vec![],
                initial_members: vec![],
            },
        }),
    };
    state.apply_event(&event);
    space
}

/// Create a space with a specific topic ID.
pub fn create_space_with_topic(state: &mut GraphState, n: u8, topic_n: u8) -> SpaceId {
    let space = make_space_id(n);
    let topic = make_topic_id(topic_n);
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id: space,
            topic_id: topic,
            space_type: SpaceType::Dao {
                initial_editors: vec![],
                initial_members: vec![],
            },
        }),
    };
    state.apply_event(&event);
    space
}

/// Add a verified edge from source to target in the graph state.
pub fn add_verified_edge(state: &mut GraphState, source: SpaceId, target: SpaceId) {
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Verified {
                target_space_id: target,
            },
        }),
    };
    state.apply_event(&event);
}

/// Add a topic edge from source to the given topic in the graph state.
pub fn add_topic_edge(state: &mut GraphState, source: SpaceId, topic: TopicId) {
    let event = SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Subtopic {
                target_topic_id: topic,
            },
        }),
    };
    state.apply_event(&event);
}

// --- Raw event factories (return events without applying) ---

/// Build a `SpaceCreated` event (does NOT apply it to state).
pub fn make_space_created_event(space_id: SpaceId, topic_id: TopicId) -> SpaceTopologyEvent {
    SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
            space_id,
            topic_id,
            space_type: SpaceType::Dao {
                initial_editors: vec![],
                initial_members: vec![],
            },
        }),
    }
}

/// Build a `Verified` trust extension event (does NOT apply it to state).
pub fn make_verified_event(source: SpaceId, target: SpaceId) -> SpaceTopologyEvent {
    SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Verified {
                target_space_id: target,
            },
        }),
    }
}

/// Build a `Subtopic` trust extension event (does NOT apply it to state).
pub fn make_subtopic_event(source: SpaceId, topic: TopicId) -> SpaceTopologyEvent {
    SpaceTopologyEvent {
        meta: make_block_meta(),
        payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
            source_space_id: source,
            extension: TrustExtension::Subtopic {
                target_topic_id: topic,
            },
        }),
    }
}
