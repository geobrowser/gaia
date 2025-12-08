//! Mock substream for testing
//!
//! Generates synthetic space topology events for testing the Atlas
//! graph processing pipeline without requiring a real blockchain connection.
//!
//! # Deterministic Topology
//!
//! The `generate_deterministic_topology()` method creates a hand-crafted graph
//! that demonstrates all topological categories:
//!
//! ```text
//! CANONICAL SPACES (reachable from Root via explicit edges):
//! =========================================================
//!
//!   Root (0x01)
//!    ├─verified─▶ A (0x0A) ─verified─▶ C (0x0C) ─verified─▶ F (0x0F)
//!    │             │                    └─related─▶ G (0x10)
//!    │             └─related─▶ D (0x0D)
//!    ├─verified─▶ B (0x0B) ─verified─▶ E (0x0E)
//!    │             └─topic[T_H]─▶ H (already canonical via explicit)
//!    └─related─▶ H (0x11) ─verified─▶ I (0x12)
//!                           └─verified─▶ J (0x13)
//!
//!   Canonical set: {Root, A, B, C, D, E, F, G, H, I, J} = 11 nodes
//!
//! NON-CANONICAL SPACES (not reachable from Root):
//! ===============================================
//!
//!   Island 1: X (0x20) ─verified─▶ Y (0x21) ─verified─▶ Z (0x22)
//!              └─related─▶ W (0x23)
//!
//!   Island 2: P (0x30) ─verified─▶ Q (0x31)
//!              └─topic[T_Q]─▶ Q
//!
//!   Island 3 (single node): S (0x40)
//!
//!   Non-canonical set: {X, Y, Z, W, P, Q, S} = 7 nodes
//!
//! TOPIC EDGES (demonstrate topic resolution):
//! ===========================================
//!
//!   - B ─topic[T_H]─▶ resolves to H (canonical) → includes H's subtree {H, I, J}
//!   - Root ─topic[T_E]─▶ resolves to E (canonical) → already in tree
//!   - X ─topic[T_A]─▶ resolves to A (canonical in Root's graph, but X isn't canonical)
//!   - P ─topic[T_Q]─▶ resolves to Q (both non-canonical)
//!
//! SHARED TOPICS (multiple spaces announce same topic):
//! ===================================================
//!
//!   Topic T_SHARED (0xF0) announced by: C, G, Y
//!   - A ─topic[T_SHARED]─▶ resolves to {C, G} (canonical), not Y (non-canonical)
//!
//! SPACES WITH EDGES POINTING TO CANONICAL (but not themselves canonical):
//! ======================================================================
//!
//!   - X ─topic[T_A]─▶ A (X not canonical, but points to canonical A)
//!   - W ─verified─▶ Root (W not canonical despite pointing to Root)
//!
//! ```

use crate::events::{
    Address, BlockMetadata, SpaceCreated, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload,
    SpaceType, TopicId, TrustExtended, TrustExtension,
};
use rand::Rng;

// ============================================================================
// Well-known IDs (deterministic across runs)
// ============================================================================

/// Root space ID - starting point for canonical graph
pub const ROOT_SPACE_ID: SpaceId = make_id(0x01);
/// Root topic ID
pub const ROOT_TOPIC_ID: TopicId = make_id(0x02);

// Canonical spaces
pub const SPACE_A: SpaceId = make_id(0x0A);
pub const SPACE_B: SpaceId = make_id(0x0B);
pub const SPACE_C: SpaceId = make_id(0x0C);
pub const SPACE_D: SpaceId = make_id(0x0D);
pub const SPACE_E: SpaceId = make_id(0x0E);
pub const SPACE_F: SpaceId = make_id(0x0F);
pub const SPACE_G: SpaceId = make_id(0x10);
pub const SPACE_H: SpaceId = make_id(0x11);
pub const SPACE_I: SpaceId = make_id(0x12);
pub const SPACE_J: SpaceId = make_id(0x13);

// Non-canonical spaces - Island 1
pub const SPACE_X: SpaceId = make_id(0x20);
pub const SPACE_Y: SpaceId = make_id(0x21);
pub const SPACE_Z: SpaceId = make_id(0x22);
pub const SPACE_W: SpaceId = make_id(0x23);

// Non-canonical spaces - Island 2
pub const SPACE_P: SpaceId = make_id(0x30);
pub const SPACE_Q: SpaceId = make_id(0x31);

// Non-canonical spaces - Island 3 (isolated)
pub const SPACE_S: SpaceId = make_id(0x40);

// Topics (each space announces its own topic, named T_<space>)
pub const TOPIC_ROOT: TopicId = ROOT_TOPIC_ID;
pub const TOPIC_A: TopicId = make_id(0x8A);
pub const TOPIC_B: TopicId = make_id(0x8B);
pub const TOPIC_C: TopicId = make_id(0x8C);
pub const TOPIC_D: TopicId = make_id(0x8D);
pub const TOPIC_E: TopicId = make_id(0x8E);
pub const TOPIC_F: TopicId = make_id(0x8F);
pub const TOPIC_G: TopicId = make_id(0x90);
pub const TOPIC_H: TopicId = make_id(0x91);
pub const TOPIC_I: TopicId = make_id(0x92);
pub const TOPIC_J: TopicId = make_id(0x93);
pub const TOPIC_X: TopicId = make_id(0xA0);
pub const TOPIC_Y: TopicId = make_id(0xA1);
pub const TOPIC_Z: TopicId = make_id(0xA2);
pub const TOPIC_W: TopicId = make_id(0xA3);
pub const TOPIC_P: TopicId = make_id(0xB0);
pub const TOPIC_Q: TopicId = make_id(0xB1);
pub const TOPIC_S: TopicId = make_id(0xC0);

// Shared topic (announced by multiple spaces: C, G, Y)
pub const TOPIC_SHARED: TopicId = make_id(0xF0);

/// Helper to create a deterministic ID from a single byte
const fn make_id(n: u8) -> [u8; 16] {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, n]
}

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
    ///
    /// The topology ensures the root space has outgoing edges so the canonical
    /// graph contains more than just the root.
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

        // Ensure root has some outgoing edges for a meaningful canonical graph
        // Connect root to ~30% of other spaces via verified edges
        if self.spaces.len() > 1 {
            let root = self.spaces[0];
            let num_root_edges = std::cmp::max(1, (self.spaces.len() - 1) * 3 / 10);
            let mut connected: Vec<usize> = Vec::new();

            for _ in 0..num_root_edges {
                let target_idx = loop {
                    let idx = rng.gen_range(1..self.spaces.len());
                    if !connected.contains(&idx) {
                        break idx;
                    }
                };
                connected.push(target_idx);
                if let Some(event) = self.create_verified_extension(root, self.spaces[target_idx]) {
                    events.push(event);
                }
            }
        }

        // Create trust relationships between non-root spaces
        // Only create edges from spaces that exist to spaces/topics that exist
        for i in 1..self.spaces.len() {
            let source = self.spaces[i];

            // Verified edges (~30% chance)
            if rng.gen_bool(0.3) && self.spaces.len() > 1 {
                let target_idx = loop {
                    let idx = rng.gen_range(0..self.spaces.len());
                    if idx != i {
                        break idx;
                    }
                };
                if let Some(event) = self.create_verified_extension(source, self.spaces[target_idx])
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
                if let Some(event) = self.create_related_extension(source, self.spaces[target_idx])
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

    /// Generate a deterministic topology that demonstrates all topological categories.
    ///
    /// This creates a hand-crafted graph with:
    /// - 11 canonical spaces (reachable from Root via explicit edges)
    /// - 7 non-canonical spaces (isolated islands)
    /// - Topic edges demonstrating topic resolution
    /// - Shared topics (multiple spaces announce the same topic)
    /// - Spaces pointing to canonical nodes but not themselves canonical
    ///
    /// See module-level documentation for the full topology diagram.
    // Allow vec_init_then_push: We can't use vec![] macro here because each push
    // calls methods that mutate `self` (incrementing block numbers, etc.)
    #[allow(clippy::vec_init_then_push)]
    pub fn generate_deterministic_topology(&mut self) -> Vec<SpaceTopologyEvent> {
        // Pre-allocate: 18 spaces + 14 explicit edges + 5 topic edges = 37 events
        let mut events = Vec::with_capacity(37);

        // =====================================================================
        // Phase 1: Create all spaces
        // =====================================================================

        // Root space (canonical)
        events.push(self.create_space_with_id(ROOT_SPACE_ID, TOPIC_ROOT));

        // Canonical spaces (will be connected to Root)
        events.push(self.create_space_with_id(SPACE_A, TOPIC_A));
        events.push(self.create_space_with_id(SPACE_B, TOPIC_B));
        events.push(self.create_space_with_id(SPACE_C, TOPIC_SHARED)); // C announces shared topic
        events.push(self.create_space_with_id(SPACE_D, TOPIC_D));
        events.push(self.create_space_with_id(SPACE_E, TOPIC_E));
        events.push(self.create_space_with_id(SPACE_F, TOPIC_F));
        events.push(self.create_space_with_id(SPACE_G, TOPIC_SHARED)); // G also announces shared topic
        events.push(self.create_space_with_id(SPACE_H, TOPIC_H));
        events.push(self.create_space_with_id(SPACE_I, TOPIC_I));
        events.push(self.create_space_with_id(SPACE_J, TOPIC_J));

        // Non-canonical spaces - Island 1
        events.push(self.create_space_with_id(SPACE_X, TOPIC_X));
        events.push(self.create_space_with_id(SPACE_Y, TOPIC_SHARED)); // Y also announces shared topic
        events.push(self.create_space_with_id(SPACE_Z, TOPIC_Z));
        events.push(self.create_space_with_id(SPACE_W, TOPIC_W));

        // Non-canonical spaces - Island 2
        events.push(self.create_space_with_id(SPACE_P, TOPIC_P));
        events.push(self.create_space_with_id(SPACE_Q, TOPIC_Q));

        // Non-canonical spaces - Island 3 (isolated single node)
        events.push(self.create_space_with_id(SPACE_S, TOPIC_S));

        // =====================================================================
        // Phase 2: Create explicit edges for canonical graph
        // =====================================================================

        // Root's direct children
        events.push(self.create_verified_extension_unchecked(ROOT_SPACE_ID, SPACE_A));
        events.push(self.create_verified_extension_unchecked(ROOT_SPACE_ID, SPACE_B));
        events.push(self.create_related_extension_unchecked(ROOT_SPACE_ID, SPACE_H));

        // A's subtree
        events.push(self.create_verified_extension_unchecked(SPACE_A, SPACE_C));
        events.push(self.create_related_extension_unchecked(SPACE_A, SPACE_D));

        // B's subtree
        events.push(self.create_verified_extension_unchecked(SPACE_B, SPACE_E));

        // C's subtree
        events.push(self.create_verified_extension_unchecked(SPACE_C, SPACE_F));
        events.push(self.create_related_extension_unchecked(SPACE_C, SPACE_G));

        // H's subtree
        events.push(self.create_verified_extension_unchecked(SPACE_H, SPACE_I));
        events.push(self.create_verified_extension_unchecked(SPACE_H, SPACE_J));

        // =====================================================================
        // Phase 3: Create explicit edges for non-canonical islands
        // =====================================================================

        // Island 1: X -> Y -> Z, X -> W
        events.push(self.create_verified_extension_unchecked(SPACE_X, SPACE_Y));
        events.push(self.create_verified_extension_unchecked(SPACE_Y, SPACE_Z));
        events.push(self.create_related_extension_unchecked(SPACE_X, SPACE_W));

        // Island 2: P -> Q
        events.push(self.create_verified_extension_unchecked(SPACE_P, SPACE_Q));

        // W points to Root (but W is still not canonical - edge goes wrong direction)
        events.push(self.create_verified_extension_unchecked(SPACE_W, ROOT_SPACE_ID));

        // =====================================================================
        // Phase 4: Create topic edges
        // =====================================================================

        // B -> topic[H] : resolves to H (canonical), adds H's subtree {I, J}
        events.push(self.create_subtopic_extension_unchecked(SPACE_B, TOPIC_H));

        // Root -> topic[E] : resolves to E (canonical), already in tree via B
        events.push(self.create_subtopic_extension_unchecked(ROOT_SPACE_ID, TOPIC_E));

        // A -> topic[SHARED] : resolves to {C, G} (canonical), Y is filtered out
        events.push(self.create_subtopic_extension_unchecked(SPACE_A, TOPIC_SHARED));

        // X -> topic[A] : X is not canonical, but points to canonical A
        // This demonstrates that pointing to canonical doesn't make you canonical
        events.push(self.create_subtopic_extension_unchecked(SPACE_X, TOPIC_A));

        // P -> topic[Q] : both P and Q are non-canonical
        events.push(self.create_subtopic_extension_unchecked(SPACE_P, TOPIC_Q));

        events
    }

    /// Create a space with a specific ID and topic
    fn create_space_with_id(&mut self, space_id: SpaceId, topic_id: TopicId) -> SpaceTopologyEvent {
        self.spaces.push(space_id);
        self.space_topics.push(topic_id);

        SpaceTopologyEvent {
            meta: self.next_block_meta(),
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

    /// Create a verified extension without validation (for deterministic topology)
    fn create_verified_extension_unchecked(
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

    /// Create a related extension without validation (for deterministic topology)
    fn create_related_extension_unchecked(
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

    /// Create a subtopic extension without validation (for deterministic topology)
    fn create_subtopic_extension_unchecked(
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
            SpaceTopologyPayload::TrustExtended(extended) => match extended.extension {
                TrustExtension::Subtopic { target_topic_id } => {
                    assert_eq!(target_topic_id, new_topic);
                }
                _ => panic!("Expected Subtopic extension"),
            },
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
