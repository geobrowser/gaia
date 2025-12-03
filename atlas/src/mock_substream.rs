//! Mock substream for testing
//!
//! Generates synthetic space topology events for testing the Atlas
//! graph processing pipeline without requiring a real blockchain connection.

use crate::events::{
    Address, BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload,
    SpaceType, TopicId, TrustExtended, TrustExtension,
};
use rand::Rng;

/// Mock substream that generates synthetic space topology events
pub struct MockSubstream {
    /// Current block number
    block_number: u64,
    /// Created spaces (for building relationships)
    spaces: Vec<SpaceId>,
    /// Topics announced by spaces
    topics: Vec<TopicId>,
    /// Cursor counter
    cursor_counter: u64,
}

impl MockSubstream {
    pub fn new() -> Self {
        Self {
            block_number: 1_000_000,
            spaces: Vec::new(),
            topics: Vec::new(),
            cursor_counter: 0,
        }
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

    /// Generate a SpaceCreated event
    pub fn create_space(&mut self) -> SpaceTopologyEvent {
        let mut rng = rand::thread_rng();

        let space_id = Self::random_uuid();
        let topic_id = Self::random_uuid();

        self.spaces.push(space_id);
        self.topics.push(topic_id);

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

    /// Generate a TrustExtended event with Verified trust
    pub fn create_verified_extension(
        &mut self,
        source: SpaceId,
        target: SpaceId,
    ) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Verified {
                    target_space_id: target,
                },
            }),
        }
    }

    /// Generate a TrustExtended event with Related trust
    pub fn create_related_extension(
        &mut self,
        source: SpaceId,
        target: SpaceId,
    ) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Related {
                    target_space_id: target,
                },
            }),
        }
    }

    /// Generate a TrustExtended event with Subtopic trust
    pub fn create_subtopic_extension(
        &mut self,
        source: SpaceId,
        target_topic: TopicId,
    ) -> SpaceTopologyEvent {
        SpaceTopologyEvent {
            meta: self.next_block_meta(),
            payload: SpaceTopologyPayload::TrustExtended(TrustExtended {
                source_space_id: source,
                extension: TrustExtension::Subtopic {
                    target_topic_id: target_topic,
                },
            }),
        }
    }

    /// Generate a batch of events simulating a realistic topology
    ///
    /// Creates `num_spaces` spaces with various trust relationships between them.
    pub fn generate_topology(&mut self, num_spaces: usize) -> Vec<SpaceTopologyEvent> {
        let mut rng = rand::thread_rng();
        let mut events = Vec::new();

        // Create spaces
        for _ in 0..num_spaces {
            events.push(self.create_space());
        }

        // Create trust relationships between spaces
        // Each space has a ~30% chance of having a verified edge to another space
        // Each space has a ~20% chance of having a related edge
        // Each space has a ~15% chance of having a subtopic edge
        for i in 0..self.spaces.len() {
            let source = self.spaces[i];

            // Verified edges
            if rng.gen_bool(0.3) && self.spaces.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.spaces.len());
                    if idx != i {
                        break idx;
                    }
                };
                events.push(self.create_verified_extension(source, self.spaces[target_idx]));
            }

            // Related edges
            if rng.gen_bool(0.2) && self.spaces.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.spaces.len());
                    if idx != i {
                        break idx;
                    }
                };
                events.push(self.create_related_extension(source, self.spaces[target_idx]));
            }

            // Subtopic edges (to other spaces' topics)
            if rng.gen_bool(0.15) && self.topics.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.topics.len());
                    if idx != i {
                        break idx;
                    }
                };
                events.push(self.create_subtopic_extension(source, self.topics[target_idx]));
            }
        }

        events
    }

    /// Get all created spaces
    pub fn spaces(&self) -> &[SpaceId] {
        &self.spaces
    }

    /// Get all topics
    pub fn topics(&self) -> &[TopicId] {
        &self.topics
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
    fn test_create_space() {
        let mut mock = MockSubstream::new();
        let event = mock.create_space();

        match event.payload {
            SpaceTopologyPayload::SpaceCreated(created) => {
                assert_eq!(mock.spaces().len(), 1);
                assert_eq!(mock.spaces()[0], created.space_id);
                assert_eq!(mock.topics()[0], created.topic_id);
            }
            _ => panic!("Expected SpaceCreated event"),
        }
    }

    #[test]
    fn test_create_verified_extension() {
        let mut mock = MockSubstream::new();

        // Create two spaces first
        mock.create_space();
        mock.create_space();

        let source = mock.spaces()[0];
        let target = mock.spaces()[1];

        let event = mock.create_verified_extension(source, target);

        match event.payload {
            SpaceTopologyPayload::TrustExtended(extended) => {
                assert_eq!(extended.source_space_id, source);
                match extended.extension {
                    TrustExtension::Verified { target_space_id } => {
                        assert_eq!(target_space_id, target);
                    }
                    _ => panic!("Expected Verified extension"),
                }
            }
            _ => panic!("Expected TrustExtended event"),
        }
    }

    #[test]
    fn test_generate_topology() {
        let mut mock = MockSubstream::new();
        let events = mock.generate_topology(10);

        // Should have at least 10 events (the space creations)
        assert!(events.len() >= 10);

        // Should have created 10 spaces
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
