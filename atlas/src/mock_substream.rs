//! Mock substream for testing
//!
//! Generates synthetic space topology events for testing the Atlas
//! graph processing pipeline without requiring a real blockchain connection.

use crate::events::{
    Address, BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload,
    SpaceType, TopicId, TrustExtended, TrustExtension,
};
use rand::Rng;

/// Well-known root space ID (deterministic across runs)
/// This is the starting point for the canonical graph.
pub const ROOT_SPACE_ID: SpaceId = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

/// Well-known root topic ID (deterministic across runs)
pub const ROOT_TOPIC_ID: TopicId = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
];

/// Mock substream that generates synthetic space topology events
pub struct MockSubstream {
    /// Current block number
    block_number: u64,
    /// Created spaces (for building relationships)
    spaces: Vec<SpaceId>,
    /// Map from space index to its announced topic
    space_topics: Vec<TopicId>,
    /// Cursor counter
    cursor_counter: u64,
}

impl MockSubstream {
    pub fn new() -> Self {
        // Start with the root space already created
        Self {
            block_number: 1_000_000,
            spaces: vec![ROOT_SPACE_ID],
            space_topics: vec![ROOT_TOPIC_ID],
            cursor_counter: 0,
        }
    }

    /// Get the root space ID
    pub fn root_space_id(&self) -> SpaceId {
        ROOT_SPACE_ID
    }

    /// Get the root topic ID
    pub fn root_topic_id(&self) -> TopicId {
        ROOT_TOPIC_ID
    }

    /// Generate a random UUID as bytes
    fn random_uuid() -> [u8; 16] {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 16];
        rng.fill(&mut bytes);
        bytes
    }

    /// Generate a random address
    fn random_address() -> Address {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 32];
        rng.fill(&mut bytes);
        bytes
    }

    /// Generate a random tx hash
    fn random_tx_hash() -> String {
        let mut rng = rand::thread_rng();
        let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
        format!("0x{}", hex::encode(bytes))
    }

    /// Get the next block metadata
    fn next_block_meta(&mut self) -> BlockMetadata {
        self.cursor_counter += 1;
        let meta = BlockMetadata {
            block_number: self.block_number,
            block_timestamp: self.block_number * 12, // ~12 second blocks
            tx_hash: Self::random_tx_hash(),
            cursor: format!("cursor_{}", self.cursor_counter),
        };
        self.block_number += 1;
        meta
    }

    /// Check if a space exists
    pub fn space_exists(&self, space_id: &SpaceId) -> bool {
        self.spaces.contains(space_id)
    }

    /// Check if a topic exists (has been announced by a space)
    pub fn topic_exists(&self, topic_id: &TopicId) -> bool {
        self.space_topics.contains(topic_id)
    }

    /// Get the topic announced by a space
    pub fn get_space_topic(&self, space_id: &SpaceId) -> Option<TopicId> {
        self.spaces
            .iter()
            .position(|s| s == space_id)
            .map(|idx| self.space_topics[idx])
    }

    /// Generate a SpaceCreated event
    pub fn create_space(&mut self) -> SpaceTopologyEvent {
        let mut rng = rand::thread_rng();

        let space_id = Self::random_uuid();
        let topic_id = Self::random_uuid();

        self.spaces.push(space_id);
        self.space_topics.push(topic_id);

        let space_type = if rng.gen_bool(0.5) {
            SpaceType::Personal {
                owner: Self::random_address(),
            }
        } else {
            let editor_count = rng.gen_range(1..=3);
            let member_count = rng.gen_range(2..=5);
            SpaceType::Dao {
                initial_editors: (0..editor_count).map(|_| Self::random_uuid()).collect(),
                initial_members: (0..member_count).map(|_| Self::random_uuid()).collect(),
            }
        };

        SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
                space_id,
                topic_id,
                space_type,
            }),
        }
    }

    /// Generate the root space creation event
    /// This should be the first event emitted.
    pub fn create_root_space(&mut self) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::SpaceCreated(SpaceCreated {
                space_id: ROOT_SPACE_ID,
                topic_id: ROOT_TOPIC_ID,
                space_type: SpaceType::Dao {
                    initial_editors: vec![],
                    initial_members: vec![],
                },
            }),
        }
    }

    /// Generate a TrustExtended event with Verified trust
    ///
    /// Returns None if source or target space doesn't exist.
    pub fn create_verified_extension(
        &mut self,
        source: SpaceId,
        target: SpaceId,
    ) -> Option<SpaceTopologyEvent> {
        if !self.space_exists(&source) || !self.space_exists(&target) {
            return None;
        }

        Some(SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Verified {
                    target_space_id: target,
                },
            }),
        })
    }

    /// Generate a TrustExtended event with Related trust
    ///
    /// Returns None if source or target space doesn't exist.
    pub fn create_related_extension(
        &mut self,
        source: SpaceId,
        target: SpaceId,
    ) -> Option<SpaceTopologyEvent> {
        if !self.space_exists(&source) || !self.space_exists(&target) {
            return None;
        }

        Some(SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Related {
                    target_space_id: target,
                },
            }),
        })
    }

    /// Generate a TrustExtended event with Subtopic trust
    ///
    /// Returns None if source space doesn't exist or target topic hasn't been announced.
    pub fn create_subtopic_extension(
        &mut self,
        source: SpaceId,
        target_topic: TopicId,
    ) -> Option<SpaceTopologyEvent> {
        if !self.space_exists(&source) || !self.topic_exists(&target_topic) {
            return None;
        }

        Some(SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Subtopic {
                    target_topic_id: target_topic,
                },
            }),
        })
    }

    /// Generate a batch of events simulating a realistic topology
    ///
    /// Creates `num_spaces` spaces with various trust relationships between them.
    /// The root space is created first, then additional spaces are created and
    /// connected with trust relationships.
    pub fn generate_topology(&mut self, num_spaces: usize) -> Vec<SpaceTopologyEvent> {
        let mut rng = rand::thread_rng();
        let mut events = Vec::new();

        // First event is always the root space creation
        events.push(self.create_root_space());

        // Create additional spaces (root already exists, so create num_spaces - 1 more)
        let additional_spaces = if num_spaces > 1 { num_spaces - 1 } else { 0 };
        for _ in 0..additional_spaces {
            events.push(self.create_space());
        }

        // Create trust relationships between spaces
        // Only create edges from spaces that exist to spaces/topics that exist
        for i in 0..self.spaces.len() {
            let source = self.spaces[i];

            // Verified edges (~30% chance)
            if rng.gen_bool(0.3) && self.spaces.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.spaces.len());
                    if idx != i {
                        break idx;
                    }
                };
                if let Some(event) =
                    self.create_verified_extension(source, self.spaces[target_idx])
                {
                    events.push(event);
                }
            }

            // Related edges (~20% chance)
            if rng.gen_bool(0.2) && self.spaces.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.spaces.len());
                    if idx != i {
                        break idx;
                    }
                };
                if let Some(event) =
                    self.create_related_extension(source, self.spaces[target_idx])
                {
                    events.push(event);
                }
            }

            // Subtopic edges to other spaces' topics (~15% chance)
            if rng.gen_bool(0.15) && self.space_topics.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.space_topics.len());
                    if idx != i {
                        break idx;
                    }
                };
                if let Some(event) =
                    self.create_subtopic_extension(source, self.space_topics[target_idx])
                {
                    events.push(event);
                }
            }
        }

        events
    }

    /// Get all created spaces
    pub fn spaces(&self) -> &[SpaceId] {
        &self.spaces
    }

    /// Get all topics (one per space)
    pub fn topics(&self) -> &[TopicId] {
        &self.space_topics
    }
}

impl Default for MockSubstream {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_starts_with_root() {
        let mock = MockSubstream::new();

        // Should start with root space and topic
        assert_eq!(mock.spaces().len(), 1);
        assert_eq!(mock.spaces()[0], ROOT_SPACE_ID);
        assert_eq!(mock.topics()[0], ROOT_TOPIC_ID);
    }

    #[test]
    fn test_root_space_is_deterministic() {
        let mock1 = MockSubstream::new();
        let mock2 = MockSubstream::new();

        assert_eq!(mock1.root_space_id(), mock2.root_space_id());
        assert_eq!(mock1.root_topic_id(), mock2.root_topic_id());
    }

    #[test]
    fn test_create_space() {
        let mut mock = MockSubstream::new();
        let event = mock.create_space();

        match event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                // Should have root + new space
                assert_eq!(mock.spaces().len(), 2);
                assert_eq!(mock.spaces()[1], created.space_id);
                assert_eq!(mock.topics()[1], created.topic_id);
            }
            _ => panic!("Expected SpaceCreated event"),
        }
    }

    #[test]
    fn test_create_verified_extension_validates_spaces() {
        let mut mock = MockSubstream::new();

        // Try to create extension with non-existent spaces
        let fake_space = [0xFFu8; 16];
        assert!(mock
            .create_verified_extension(fake_space, ROOT_SPACE_ID)
            .is_none());
        assert!(mock
            .create_verified_extension(ROOT_SPACE_ID, fake_space)
            .is_none());

        // Create a real space and verify extension works
        mock.create_space();
        let new_space = mock.spaces()[1];

        let event = mock
            .create_verified_extension(ROOT_SPACE_ID, new_space)
            .expect("Should create extension between existing spaces");

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => {
                assert_eq!(extended.source_space_id, ROOT_SPACE_ID);
                match extended.extension {
                    TrustExtension::Verified { target_space_id } => {
                        assert_eq!(target_space_id, new_space);
                    }
                    _ => panic!("Expected Verified extension"),
                }
            }
            _ => panic!("Expected TrustExtended event"),
        }
    }

    #[test]
    fn test_create_subtopic_extension_validates_topic() {
        let mut mock = MockSubstream::new();

        // Try to create subtopic extension with non-existent topic
        let fake_topic = [0xFFu8; 16];
        assert!(mock
            .create_subtopic_extension(ROOT_SPACE_ID, fake_topic)
            .is_none());

        // Create a space (which announces a topic)
        mock.create_space();
        let new_topic = mock.topics()[1];

        // Now the subtopic extension should work
        let event = mock
            .create_subtopic_extension(ROOT_SPACE_ID, new_topic)
            .expect("Should create subtopic extension to existing topic");

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => {
                match extended.extension {
                    TrustExtension::Subtopic { target_topic_id } => {
                        assert_eq!(target_topic_id, new_topic);
                    }
                    _ => panic!("Expected Subtopic extension"),
                }
            }
            _ => panic!("Expected TrustExtended event"),
        }
    }

    #[test]
    fn test_generate_topology_starts_with_root() {
        let mut mock = MockSubstream::new();
        let events = mock.generate_topology(10);

        // First event should be root space creation
        match &events[0].payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(created.space_id, ROOT_SPACE_ID);
                assert_eq!(created.topic_id, ROOT_TOPIC_ID);
            }
            _ => panic!("First event should be root space creation"),
        }

        // Should have 10 spaces total (root + 9 additional)
        assert_eq!(mock.spaces().len(), 10);
        assert_eq!(mock.topics().len(), 10);

        // Count event types
        let space_created_count = events
            .iter()
            .filter(|e| matches!(e.payload, SpaceTopologyPayload::SpaceCreated(_)))
            .count();
        let trust_extended_count = events
            .iter()
            .filter(|e| matches!(e.payload, SpaceTopologyPayload::TrustExtended(_)))
            .count();

        assert_eq!(space_created_count, 10);
        println!(
            "Generated {} spaces and {} trust extensions",
            space_created_count, trust_extended_count
        );
    }
}
