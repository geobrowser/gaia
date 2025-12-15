//! Mock substream generator for producing test events.
//!
//! Supports both deterministic mode (for reproducible tests) and random mode
//! (for fuzz testing and load testing).

use crate::events::*;

/// Configuration for the mock substream generator.
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Whether to generate deterministic (reproducible) data.
    /// When true, the same sequence of events is generated each time.
    pub deterministic: bool,
    /// Whether to include edit events (GRC-20 operations).
    pub include_edits: bool,
    /// Number of spaces to generate.
    pub num_spaces: usize,
    /// Number of edits per space (only used if `include_edits` is true).
    pub edits_per_space: usize,
    /// Starting block number.
    pub start_block: u64,
    /// Starting timestamp (unix seconds).
    pub start_timestamp: u64,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            deterministic: true,
            include_edits: false,
            num_spaces: 10,
            edits_per_space: 5,
            start_block: 1_000_000,
            start_timestamp: 1_700_000_000,
        }
    }
}

impl MockConfig {
    /// Create a new config for deterministic testing.
    pub fn deterministic() -> Self {
        Self {
            deterministic: true,
            ..Default::default()
        }
    }

    /// Create a new config with edits enabled.
    pub fn with_edits(mut self) -> Self {
        self.include_edits = true;
        self
    }

    /// Set the number of spaces.
    pub fn with_num_spaces(mut self, num_spaces: usize) -> Self {
        self.num_spaces = num_spaces;
        self
    }

    /// Set the number of edits per space.
    pub fn with_edits_per_space(mut self, edits_per_space: usize) -> Self {
        self.edits_per_space = edits_per_space;
        self
    }
}

/// A mock substream that generates blockchain events.
#[derive(Debug)]
pub struct MockSubstream {
    config: MockConfig,
    current_block: u64,
    current_timestamp: u64,
    event_counter: u64,
}

impl MockSubstream {
    /// Create a new mock substream with the given configuration.
    pub fn new(config: MockConfig) -> Self {
        Self {
            current_block: config.start_block,
            current_timestamp: config.start_timestamp,
            event_counter: 0,
            config,
        }
    }

    /// Create a mock substream with default deterministic configuration.
    pub fn deterministic() -> Self {
        Self::new(MockConfig::deterministic())
    }

    /// Generate the next block of events.
    ///
    /// In deterministic mode, this generates a predictable sequence.
    /// In random mode (requires `random` feature), this generates random events.
    pub fn next_block(&mut self) -> MockBlock {
        let block = MockBlock {
            number: self.current_block,
            timestamp: self.current_timestamp,
            cursor: format!("cursor_{}", self.current_block),
            events: Vec::new(),
        };

        // Advance state
        self.current_block += 1;
        self.current_timestamp += 12; // ~12 second block time

        block
    }

    /// Generate a block with specific events.
    pub fn block_with_events(&mut self, events: Vec<MockEvent>) -> MockBlock {
        let block = MockBlock {
            number: self.current_block,
            timestamp: self.current_timestamp,
            cursor: format!("cursor_{}", self.current_block),
            events,
        };

        self.current_block += 1;
        self.current_timestamp += 12;

        block
    }

    /// Create metadata for the current block state.
    pub fn current_metadata(&self) -> BlockMetadata {
        BlockMetadata {
            block_number: self.current_block,
            block_timestamp: self.current_timestamp,
            tx_hash: format!("0x{:064x}", self.event_counter),
            cursor: format!("cursor_{}", self.current_block),
        }
    }

    /// Create a space creation event.
    pub fn create_space(&mut self, space_id: SpaceId, topic_id: TopicId, space_type: SpaceType) -> SpaceCreated {
        self.event_counter += 1;
        SpaceCreated {
            meta: self.current_metadata(),
            space_id,
            topic_id,
            space_type,
        }
    }

    /// Create a personal space creation event.
    pub fn create_personal_space(&mut self, space_id: SpaceId, topic_id: TopicId, owner: Address) -> SpaceCreated {
        self.create_space(space_id, topic_id, SpaceType::Personal { owner })
    }

    /// Create a DAO space creation event.
    pub fn create_dao_space(
        &mut self,
        space_id: SpaceId,
        topic_id: TopicId,
        initial_editors: Vec<SpaceId>,
        initial_members: Vec<SpaceId>,
    ) -> SpaceCreated {
        self.create_space(
            space_id,
            topic_id,
            SpaceType::Dao {
                initial_editors,
                initial_members,
            },
        )
    }

    /// Create a trust extension event.
    pub fn extend_trust(&mut self, source_space_id: SpaceId, extension: TrustExtension) -> TrustExtended {
        self.event_counter += 1;
        TrustExtended {
            meta: self.current_metadata(),
            source_space_id,
            extension,
        }
    }

    /// Create a verified trust extension.
    pub fn extend_verified(&mut self, source: SpaceId, target: SpaceId) -> TrustExtended {
        self.extend_trust(source, TrustExtension::Verified { target_space_id: target })
    }

    /// Create a related trust extension.
    pub fn extend_related(&mut self, source: SpaceId, target: SpaceId) -> TrustExtended {
        self.extend_trust(source, TrustExtension::Related { target_space_id: target })
    }

    /// Create a subtopic trust extension.
    pub fn extend_subtopic(&mut self, source: SpaceId, target_topic: TopicId) -> TrustExtended {
        self.extend_trust(source, TrustExtension::Subtopic { target_topic_id: target_topic })
    }

    /// Create an edit published event.
    pub fn publish_edit(
        &mut self,
        edit_id: EditId,
        space_id: SpaceId,
        authors: Vec<Address>,
        name: String,
        ops: Vec<Op>,
    ) -> EditPublished {
        self.event_counter += 1;
        EditPublished {
            meta: self.current_metadata(),
            edit_id,
            space_id,
            authors,
            name,
            ops,
        }
    }

    /// Get the current block number.
    pub fn current_block_number(&self) -> u64 {
        self.current_block
    }

    /// Get the current timestamp.
    pub fn current_timestamp(&self) -> u64 {
        self.current_timestamp
    }

    /// Get the configuration.
    pub fn config(&self) -> &MockConfig {
        &self.config
    }
}

#[cfg(feature = "random")]
mod random_impl {
    use super::*;
    use rand::Rng;

    impl MockSubstream {
        /// Generate a random space ID.
        pub fn random_space_id<R: Rng>(rng: &mut R) -> SpaceId {
            let mut id = [0u8; 16];
            rng.fill(&mut id);
            id
        }

        /// Generate a random topic ID.
        pub fn random_topic_id<R: Rng>(rng: &mut R) -> TopicId {
            Self::random_space_id(rng)
        }

        /// Generate a random address.
        pub fn random_address<R: Rng>(rng: &mut R) -> Address {
            let mut addr = [0u8; 32];
            rng.fill(&mut addr);
            addr
        }

        /// Generate a random edit ID.
        pub fn random_edit_id<R: Rng>(rng: &mut R) -> EditId {
            Self::random_space_id(rng)
        }

        /// Generate random events based on the configuration.
        pub fn generate_random_topology<R: Rng>(&mut self, rng: &mut R) -> Vec<MockBlock> {
            let mut blocks = Vec::new();
            let mut spaces: Vec<(SpaceId, TopicId)> = Vec::new();

            // Generate spaces
            for _ in 0..self.config.num_spaces {
                let space_id = Self::random_space_id(rng);
                let topic_id = Self::random_topic_id(rng);

                let space_type = if rng.gen_bool(0.5) {
                    SpaceType::Personal {
                        owner: Self::random_address(rng),
                    }
                } else {
                    let num_editors = rng.gen_range(1..=5);
                    let num_members = rng.gen_range(3..=10);
                    SpaceType::Dao {
                        initial_editors: (0..num_editors).map(|_| Self::random_space_id(rng)).collect(),
                        initial_members: (0..num_members).map(|_| Self::random_space_id(rng)).collect(),
                    }
                };

                let event = self.create_space(space_id, topic_id, space_type);
                blocks.push(self.block_with_events(vec![MockEvent::SpaceCreated(event)]));
                spaces.push((space_id, topic_id));
            }

            // Generate trust edges
            for i in 0..spaces.len() {
                let source = spaces[i].0;

                // 30% chance of verified edge
                if rng.gen_bool(0.3) && i + 1 < spaces.len() {
                    let target_idx = rng.gen_range(0..spaces.len());
                    if target_idx != i {
                        let event = self.extend_verified(source, spaces[target_idx].0);
                        blocks.push(self.block_with_events(vec![MockEvent::TrustExtended(event)]));
                    }
                }

                // 20% chance of related edge
                if rng.gen_bool(0.2) {
                    let target_idx = rng.gen_range(0..spaces.len());
                    if target_idx != i {
                        let event = self.extend_related(source, spaces[target_idx].0);
                        blocks.push(self.block_with_events(vec![MockEvent::TrustExtended(event)]));
                    }
                }

                // 15% chance of subtopic edge
                if rng.gen_bool(0.15) {
                    let target_idx = rng.gen_range(0..spaces.len());
                    let event = self.extend_subtopic(source, spaces[target_idx].1);
                    blocks.push(self.block_with_events(vec![MockEvent::TrustExtended(event)]));
                }
            }

            // Generate edits if configured
            if self.config.include_edits {
                for (space_id, _) in &spaces {
                    for j in 0..self.config.edits_per_space {
                        let edit_id = Self::random_edit_id(rng);
                        let author = Self::random_address(rng);
                        let ops = self.generate_random_ops(rng);

                        let event = self.publish_edit(
                            edit_id,
                            *space_id,
                            vec![author],
                            format!("Edit {}", j),
                            ops,
                        );
                        blocks.push(self.block_with_events(vec![MockEvent::EditPublished(event)]));
                    }
                }
            }

            blocks
        }

        /// Generate a random 16-byte ID.
        fn random_id<R: Rng>(rng: &mut R) -> [u8; 16] {
            let mut id = [0u8; 16];
            rng.fill(&mut id);
            id
        }

        fn generate_random_ops<R: Rng>(&self, rng: &mut R) -> Vec<Op> {
            let num_ops = rng.gen_range(1..=5);
            let mut ops = Vec::with_capacity(num_ops);
            let mut entities: Vec<EntityId> = Vec::new();

            for _ in 0..num_ops {
                let op = match rng.gen_range(0..3) {
                    0 => {
                        // UpdateEntity
                        let entity_id = Self::random_id(rng);
                        entities.push(entity_id);
                        Op::UpdateEntity(UpdateEntity {
                            id: entity_id,
                            values: vec![Value {
                                property: Self::random_id(rng),
                                value: format!("value_{}", rng.gen::<u32>()),
                            }],
                        })
                    }
                    1 => {
                        // CreateProperty
                        Op::CreateProperty(CreateProperty {
                            id: Self::random_id(rng),
                            data_type: DataType::String,
                        })
                    }
                    _ => {
                        // CreateRelation
                        let from_entity = if entities.is_empty() {
                            Self::random_id(rng)
                        } else {
                            entities[rng.gen_range(0..entities.len())]
                        };
                        let to_entity = Self::random_id(rng);
                        Op::CreateRelation(CreateRelation {
                            id: Self::random_id(rng),
                            relation_type: Self::random_id(rng),
                            from_entity,
                            from_space: None,
                            to_entity,
                            to_space: None,
                            entity: Self::random_id(rng),
                            position: None,
                            verified: Some(true),
                        })
                    }
                };
                ops.push(op);
            }

            ops
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_substream_creation() {
        let mock = MockSubstream::deterministic();
        assert_eq!(mock.current_block_number(), 1_000_000);
    }

    #[test]
    fn test_next_block_advances_state() {
        let mut mock = MockSubstream::deterministic();
        let block1 = mock.next_block();
        let block2 = mock.next_block();

        assert_eq!(block1.number, 1_000_000);
        assert_eq!(block2.number, 1_000_001);
        assert_eq!(block2.timestamp - block1.timestamp, 12);
    }

    #[test]
    fn test_create_personal_space() {
        let mut mock = MockSubstream::deterministic();
        let space_id = make_id(0x01);
        let topic_id = make_id(0x02);
        let owner = make_address(0xAA);

        let event = mock.create_personal_space(space_id, topic_id, owner);

        assert_eq!(event.space_id, space_id);
        assert_eq!(event.topic_id, topic_id);
        match event.space_type {
            SpaceType::Personal { owner: o } => assert_eq!(o, owner),
            _ => panic!("Expected personal space"),
        }
    }

    #[test]
    fn test_extend_trust() {
        let mut mock = MockSubstream::deterministic();
        let source = make_id(0x01);
        let target = make_id(0x02);

        let verified = mock.extend_verified(source, target);
        assert_eq!(verified.source_space_id, source);
        match verified.extension {
            TrustExtension::Verified { target_space_id } => assert_eq!(target_space_id, target),
            _ => panic!("Expected verified extension"),
        }

        let related = mock.extend_related(source, target);
        match related.extension {
            TrustExtension::Related { target_space_id } => assert_eq!(target_space_id, target),
            _ => panic!("Expected related extension"),
        }

        let subtopic = mock.extend_subtopic(source, target);
        match subtopic.extension {
            TrustExtension::Subtopic { target_topic_id } => assert_eq!(target_topic_id, target),
            _ => panic!("Expected subtopic extension"),
        }
    }
}
