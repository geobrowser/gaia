//! Graph state for topology storage
//!
//! `GraphState` is the in-memory representation of the space topology graph,
//! updated by processing blockchain events.

use crate::events::{
    SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload, TopicId, TrustExtended,
    TrustExtension,
};
use std::collections::{HashMap, HashSet};

use super::EdgeType;

/// In-memory state of the topology graph
#[derive(Debug, Default)]
pub struct GraphState {
    /// All known spaces
    pub spaces: HashSet<SpaceId>,

    /// Topic announced by each space (space_id -> topic_id)
    pub space_topics: HashMap<SpaceId, TopicId>,

    /// Reverse mapping: topic -> spaces that announced it
    pub topic_spaces: HashMap<TopicId, HashSet<SpaceId>>,

    /// Explicit edges: source -> [(target, edge_type)]
    pub explicit_edges: HashMap<SpaceId, Vec<(SpaceId, EdgeType)>>,

    /// Topic edges: source -> [topic_ids]
    pub topic_edges: HashMap<SpaceId, HashSet<TopicId>>,

    /// Reverse topic edges: topic -> spaces that have edges TO this topic
    /// Used for O(1) lookup of which spaces are affected when a topic changes
    pub topic_edge_sources: HashMap<TopicId, HashSet<SpaceId>>,
}

impl GraphState {
    /// Create a new empty graph state
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a topology event to update the graph state
    pub fn apply_event(&mut self, event: &SpaceTopologyEvent) {
        match &event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                self.apply_space_created(created);
            }
            SpaceTopologyPayload::TrustExtended(extended) => {
                self.apply_trust_extended(extended);
            }
        }
    }

    /// Apply a SpaceCreated event
    fn apply_space_created(&mut self, event: &SpaceCreated) {
        // Add space to known spaces
        self.spaces.insert(event.space_id);

        // Record the topic this space announces
        self.space_topics.insert(event.space_id, event.topic_id);

        // Add to reverse topic mapping
        self.topic_spaces
            .entry(event.topic_id)
            .or_default()
            .insert(event.space_id);
    }

    /// Apply a TrustExtended event
    fn apply_trust_extended(&mut self, event: &TrustExtended) {
        let source = event.source_space_id;

        match &event.extension {
            TrustExtension::Verified { target_space_id } => {
                self.explicit_edges
                    .entry(source)
                    .or_default()
                    .push((*target_space_id, EdgeType::Verified));
            }
            TrustExtension::Related { target_space_id } => {
                self.explicit_edges
                    .entry(source)
                    .or_default()
                    .push((*target_space_id, EdgeType::Related));
            }
            TrustExtension::Subtopic { target_topic_id } => {
                self.topic_edges
                    .entry(source)
                    .or_default()
                    .insert(*target_topic_id);

                // Maintain reverse index for O(1) lookup
                self.topic_edge_sources
                    .entry(*target_topic_id)
                    .or_default()
                    .insert(source);
            }
        }
    }

    /// Check if a space exists in the graph
    pub fn contains_space(&self, space_id: &SpaceId) -> bool {
        self.spaces.contains(space_id)
    }

    /// Get the topic announced by a space
    pub fn get_space_topic(&self, space_id: &SpaceId) -> Option<&TopicId> {
        self.space_topics.get(space_id)
    }

    /// Get all spaces that announced a topic
    pub fn get_topic_members(&self, topic_id: &TopicId) -> Option<&HashSet<SpaceId>> {
        self.topic_spaces.get(topic_id)
    }

    /// Get explicit edges from a space
    pub fn get_explicit_edges(&self, space_id: &SpaceId) -> Option<&Vec<(SpaceId, EdgeType)>> {
        self.explicit_edges.get(space_id)
    }

    /// Get topic edges from a space
    pub fn get_topic_edges(&self, space_id: &SpaceId) -> Option<&HashSet<TopicId>> {
        self.topic_edges.get(space_id)
    }

    /// Get all spaces that have a topic edge TO the given topic (O(1) lookup)
    pub fn get_topic_edge_sources(&self, topic_id: &TopicId) -> Option<&HashSet<SpaceId>> {
        self.topic_edge_sources.get(topic_id)
    }

    /// Get total number of spaces
    pub fn space_count(&self) -> usize {
        self.spaces.len()
    }

    /// Get total number of explicit edges
    pub fn explicit_edge_count(&self) -> usize {
        self.explicit_edges.values().map(|v| v.len()).sum()
    }

    /// Get total number of topic edges
    pub fn topic_edge_count(&self) -> usize {
        self.topic_edges.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BlockMetadata, SpaceType};

    fn make_space_id(n: u8) -> SpaceId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_topic_id(n: u8) -> TopicId {
        let mut id = [0u8; 16];
        id[15] = n;
        id
    }

    fn make_block_meta(block: u64) -> BlockMetadata {
        BlockMetadata {
            block_number: block,
            block_timestamp: block * 12,
            tx_hash: format!("0x{:064x}", block),
            cursor: format!("cursor_{}", block),
        }
    }

    fn make_space_created_event(space_id: SpaceId, topic_id: TopicId) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: make_block_meta(1),
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

    fn make_verified_event(source: SpaceId, target: SpaceId) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: make_block_meta(2),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Verified {
                    target_space_id: target,
                },
            }),
        }
    }

    fn make_subtopic_event(source: SpaceId, topic: TopicId) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: make_block_meta(3),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Subtopic {
                    target_topic_id: topic,
                },
            }),
        }
    }

    #[test]
    fn test_new_state_is_empty() {
        let state = GraphState::new();
        assert_eq!(state.space_count(), 0);
        assert_eq!(state.explicit_edge_count(), 0);
        assert_eq!(state.topic_edge_count(), 0);
    }

    #[test]
    fn test_apply_space_created() {
        let mut state = GraphState::new();
        let space = make_space_id(1);
        let topic = make_topic_id(1);

        state.apply_event(&make_space_created_event(space, topic));

        assert!(state.contains_space(&space));
        assert_eq!(state.get_space_topic(&space), Some(&topic));
        assert!(state.get_topic_members(&topic).unwrap().contains(&space));
    }

    #[test]
    fn test_apply_verified_edge() {
        let mut state = GraphState::new();
        let space1 = make_space_id(1);
        let space2 = make_space_id(2);

        state.apply_event(&make_space_created_event(space1, make_topic_id(1)));
        state.apply_event(&make_space_created_event(space2, make_topic_id(2)));
        state.apply_event(&make_verified_event(space1, space2));

        let edges = state.get_explicit_edges(&space1).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0], (space2, EdgeType::Verified));
    }

    #[test]
    fn test_apply_subtopic_edge() {
        let mut state = GraphState::new();
        let space1 = make_space_id(1);
        let space2 = make_space_id(2);
        let topic2 = make_topic_id(2);

        state.apply_event(&make_space_created_event(space1, make_topic_id(1)));
        state.apply_event(&make_space_created_event(space2, topic2));
        state.apply_event(&make_subtopic_event(space1, topic2));

        let topic_edges = state.get_topic_edges(&space1).unwrap();
        assert!(topic_edges.contains(&topic2));
    }

    #[test]
    fn test_topic_members() {
        let mut state = GraphState::new();
        let topic = make_topic_id(1);

        // Two spaces announce the same topic
        let space1 = make_space_id(1);
        let space2 = make_space_id(2);

        state.apply_event(&make_space_created_event(space1, topic));
        state.apply_event(&make_space_created_event(space2, topic));

        let members = state.get_topic_members(&topic).unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.contains(&space1));
        assert!(members.contains(&space2));
    }
}
