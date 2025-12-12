//! Mock event builders mirroring mock-substream's event types.
//!
//! These builders create `Action` events in the chain format, matching
//! the semantic events from `mock-substream`:
//!
//! - `SpaceCreated` → `SPACE_REGISTERED` action
//! - `TrustExtended` (Verified/Related/Subtopic) → `SUBSPACE_ADDED` action
//! - `EditPublished` → `EDITS_PUBLISHED` action
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::events;
//!
//! let actions = vec![
//!     // Create a personal space
//!     events::space_created([0x01; 16], [0xaa; 32]),
//!     // Extend verified trust
//!     events::trust_extended_verified([0x01; 16], [0x02; 16]),
//!     // Publish edits with IPFS hash
//!     events::edit_published([0x01; 16], "QmYwAPJzv5CZsnANOTaREALhashhere"),
//! ];
//! ```

use crate::actions;
use hermes_substream::pb::hermes::Action;

// Trust extension type bytes (first 2 bytes of data field)
const TRUST_TYPE_VERIFIED: [u8; 2] = [0x00, 0x00];
const TRUST_TYPE_RELATED: [u8; 2] = [0x00, 0x01];
const TRUST_TYPE_SUBTOPIC: [u8; 2] = [0x00, 0x02];

// =============================================================================
// Type aliases (matching mock-substream)
// =============================================================================

pub type SpaceId = [u8; 16];
pub type TopicId = [u8; 16];
pub type Address = [u8; 32];

// =============================================================================
// SpaceCreated -> SPACE_REGISTERED
// =============================================================================

/// Create a SPACE_REGISTERED action (SpaceCreated event).
///
/// Maps to mock-substream's `SpaceCreated` with `SpaceType::Personal`.
///
/// - `space_id`: The 16-byte ID of the new space
/// - `owner`: The 32-byte owner address
pub fn space_created(space_id: SpaceId, owner: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SPACE_REGISTERED.to_vec(),
        topic: owner.to_vec(),
        data: vec![],
    }
}

/// Create a SPACE_REGISTERED action for a DAO space.
///
/// Maps to mock-substream's `SpaceCreated` with `SpaceType::Dao`.
///
/// - `space_id`: The 16-byte ID of the new space  
/// - `initial_editors`: List of initial editor space IDs
/// - `initial_members`: List of initial member space IDs
pub fn space_created_dao(
    space_id: SpaceId,
    initial_editors: Vec<SpaceId>,
    initial_members: Vec<SpaceId>,
) -> Action {
    // Encode editors and members into data field
    let mut data = Vec::new();

    // Number of editors (2 bytes)
    data.extend_from_slice(&(initial_editors.len() as u16).to_be_bytes());
    for editor in &initial_editors {
        data.extend_from_slice(editor);
    }

    // Number of members (2 bytes)
    data.extend_from_slice(&(initial_members.len() as u16).to_be_bytes());
    for member in &initial_members {
        data.extend_from_slice(member);
    }

    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SPACE_REGISTERED.to_vec(),
        topic: vec![0u8; 32], // No owner for DAO
        data,
    }
}

// =============================================================================
// TrustExtended -> SUBSPACE_ADDED
// =============================================================================

/// Create a SUBSPACE_ADDED action for verified trust.
///
/// Maps to mock-substream's `TrustExtended` with `TrustExtension::Verified`.
///
/// - `source_space_id`: The space extending trust
/// - `target_space_id`: The space being verified
pub fn trust_extended_verified(source_space_id: SpaceId, target_space_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&target_space_id);

    Action {
        from_id: source_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_ADDED.to_vec(),
        topic,
        data: TRUST_TYPE_VERIFIED.to_vec(),
    }
}

/// Create a SUBSPACE_ADDED action for related trust.
///
/// Maps to mock-substream's `TrustExtended` with `TrustExtension::Related`.
///
/// - `source_space_id`: The space extending trust
/// - `target_space_id`: The related space
pub fn trust_extended_related(source_space_id: SpaceId, target_space_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&target_space_id);

    Action {
        from_id: source_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_ADDED.to_vec(),
        topic,
        data: TRUST_TYPE_RELATED.to_vec(),
    }
}

/// Create a SUBSPACE_ADDED action for subtopic trust.
///
/// Maps to mock-substream's `TrustExtended` with `TrustExtension::Subtopic`.
///
/// - `source_space_id`: The space extending trust
/// - `target_topic_id`: The subtopic's topic ID
pub fn trust_extended_subtopic(source_space_id: SpaceId, target_topic_id: TopicId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&target_topic_id);

    Action {
        from_id: source_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_ADDED.to_vec(),
        topic,
        data: TRUST_TYPE_SUBTOPIC.to_vec(),
    }
}

// =============================================================================
// EditPublished -> EDITS_PUBLISHED
// =============================================================================

/// Create an EDITS_PUBLISHED action.
///
/// Maps to mock-substream's `EditPublished`.
///
/// - `space_id`: The space publishing the edit
/// - `ipfs_hash`: The IPFS hash of the edit content (e.g., "QmYwAPJzv5CZsnA...")
pub fn edit_published(space_id: SpaceId, ipfs_hash: &str) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::EDITS_PUBLISHED.to_vec(),
        topic: vec![0u8; 32],
        data: ipfs_hash.as_bytes().to_vec(),
    }
}

// =============================================================================
// Helper functions (matching mock-substream)
// =============================================================================

/// Helper to create a well-known ID from a single byte.
///
/// Creates an ID with all zeros except the last byte.
/// Example: `make_id(0x0A)` produces `[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0x0A]`
pub const fn make_id(last_byte: u8) -> SpaceId {
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, last_byte]
}

/// Helper to create a well-known address from a single byte.
///
/// Creates an address with all zeros except the last byte.
pub const fn make_address(last_byte: u8) -> Address {
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, last_byte,
    ]
}

// =============================================================================
// Convenience: Generate test topology matching mock-substream
// =============================================================================

/// Well-known space IDs (matching mock-substream::test_topology)
pub mod test_topology {
    use super::*;

    pub const ROOT_SPACE_ID: SpaceId = make_id(0x01);
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

    // Non-canonical spaces
    pub const SPACE_X: SpaceId = make_id(0x20);
    pub const SPACE_Y: SpaceId = make_id(0x21);
    pub const SPACE_Z: SpaceId = make_id(0x22);
    pub const SPACE_W: SpaceId = make_id(0x23);
    pub const SPACE_P: SpaceId = make_id(0x30);
    pub const SPACE_Q: SpaceId = make_id(0x31);
    pub const SPACE_S: SpaceId = make_id(0x40);

    // Topic IDs
    pub const ROOT_TOPIC_ID: TopicId = make_id(0x02);
    pub const TOPIC_A: TopicId = make_id(0x8A);
    pub const TOPIC_B: TopicId = make_id(0x8B);
    pub const TOPIC_H: TopicId = make_id(0x91);
    pub const TOPIC_E: TopicId = make_id(0x8E);
    pub const TOPIC_Q: TopicId = make_id(0xB1);
    pub const TOPIC_SHARED: TopicId = make_id(0xF0);

    // Addresses
    pub const ROOT_OWNER: Address = make_address(0x01);
    pub const USER_1: Address = make_address(0x11);
    pub const USER_2: Address = make_address(0x12);
    pub const USER_3: Address = make_address(0x13);

    /// Generate all events matching mock-substream's test_topology::generate().
    ///
    /// Returns actions for:
    /// - 18 space creations (11 canonical + 7 non-canonical)
    /// - 14 explicit trust edges + 5 topic edges
    /// - 6 edit events
    #[allow(clippy::vec_init_then_push)]
    pub fn generate() -> Vec<Action> {
        let mut actions = Vec::new();

        // Phase 1: Create all spaces
        actions.push(space_created(ROOT_SPACE_ID, ROOT_OWNER));
        actions.push(space_created(SPACE_A, USER_1));
        actions.push(space_created(SPACE_B, USER_2));
        actions.push(space_created(SPACE_C, USER_1));
        actions.push(space_created(SPACE_D, USER_2));
        actions.push(space_created(SPACE_E, USER_3));
        actions.push(space_created(SPACE_F, USER_1));
        actions.push(space_created(SPACE_G, USER_2));
        actions.push(space_created(SPACE_H, USER_3));
        actions.push(space_created(SPACE_I, USER_1));
        actions.push(space_created(SPACE_J, USER_2));

        // Non-canonical - Island 1
        actions.push(space_created(SPACE_X, USER_1));
        actions.push(space_created(SPACE_Y, USER_2));
        actions.push(space_created(SPACE_Z, USER_3));
        actions.push(space_created(SPACE_W, USER_1));

        // Non-canonical - Island 2 (P is DAO)
        actions.push(space_created_dao(SPACE_P, vec![SPACE_Q], vec![]));
        actions.push(space_created(SPACE_Q, USER_2));

        // Non-canonical - Island 3
        actions.push(space_created(SPACE_S, USER_3));

        // Phase 2: Trust edges (canonical graph)
        actions.push(trust_extended_verified(ROOT_SPACE_ID, SPACE_A));
        actions.push(trust_extended_verified(ROOT_SPACE_ID, SPACE_B));
        actions.push(trust_extended_related(ROOT_SPACE_ID, SPACE_H));

        actions.push(trust_extended_verified(SPACE_A, SPACE_C));
        actions.push(trust_extended_related(SPACE_A, SPACE_D));

        actions.push(trust_extended_verified(SPACE_B, SPACE_E));

        actions.push(trust_extended_verified(SPACE_C, SPACE_F));
        actions.push(trust_extended_related(SPACE_C, SPACE_G));

        actions.push(trust_extended_verified(SPACE_H, SPACE_I));
        actions.push(trust_extended_verified(SPACE_H, SPACE_J));

        // Phase 3: Trust edges (non-canonical islands)
        actions.push(trust_extended_verified(SPACE_X, SPACE_Y));
        actions.push(trust_extended_related(SPACE_X, SPACE_W));
        actions.push(trust_extended_verified(SPACE_Y, SPACE_Z));
        actions.push(trust_extended_verified(SPACE_P, SPACE_Q));

        // Phase 4: Topic-based trust edges
        actions.push(trust_extended_subtopic(SPACE_B, TOPIC_H));
        actions.push(trust_extended_subtopic(ROOT_SPACE_ID, TOPIC_E));
        actions.push(trust_extended_subtopic(SPACE_A, TOPIC_SHARED));
        actions.push(trust_extended_subtopic(SPACE_X, TOPIC_A));
        actions.push(trust_extended_subtopic(SPACE_P, TOPIC_Q));

        // Phase 5: Edits
        actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit1CreatePersons"));
        actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit2AddDescriptions"));
        actions.push(edit_published(SPACE_A, "QmSpaceAEdit1CreateOrg"));
        actions.push(edit_published(SPACE_A, "QmSpaceAEdit2CreateRelations"));
        actions.push(edit_published(SPACE_B, "QmSpaceBEdit1CreateDoc"));
        actions.push(edit_published(SPACE_C, "QmSpaceCEdit1CreateTopic"));

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_created_format() {
        let space_id = make_id(0x01);
        let owner = make_address(0xaa);
        let action = space_created(space_id, owner);

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.action, actions::SPACE_REGISTERED.to_vec());
        assert_eq!(action.topic, owner.to_vec());
    }

    #[test]
    fn test_trust_extended_verified_format() {
        let source = make_id(0x01);
        let target = make_id(0x02);
        let action = trust_extended_verified(source, target);

        assert_eq!(action.from_id, source.to_vec());
        assert_eq!(action.action, actions::SUBSPACE_ADDED.to_vec());
        assert_eq!(&action.topic[16..32], &target);
        assert_eq!(action.data, TRUST_TYPE_VERIFIED.to_vec());
    }

    #[test]
    fn test_trust_extended_related_format() {
        let source = make_id(0x01);
        let target = make_id(0x02);
        let action = trust_extended_related(source, target);

        assert_eq!(action.data, TRUST_TYPE_RELATED.to_vec());
    }

    #[test]
    fn test_trust_extended_subtopic_format() {
        let source = make_id(0x01);
        let topic = make_id(0x02);
        let action = trust_extended_subtopic(source, topic);

        assert_eq!(action.data, TRUST_TYPE_SUBTOPIC.to_vec());
        assert_eq!(&action.topic[16..32], &topic);
    }

    #[test]
    fn test_edit_published_format() {
        let space_id = make_id(0x01);
        let ipfs_hash = "QmYwAPJzv5CZsnANOTaREALhashhere";
        let action = edit_published(space_id, ipfs_hash);

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.action, actions::EDITS_PUBLISHED.to_vec());
        assert_eq!(action.data, ipfs_hash.as_bytes());
    }

    #[test]
    fn test_topology_generate_counts() {
        let actions = test_topology::generate();

        let space_count = actions
            .iter()
            .filter(|a| a.action == actions::SPACE_REGISTERED.to_vec())
            .count();
        let trust_count = actions
            .iter()
            .filter(|a| a.action == actions::SUBSPACE_ADDED.to_vec())
            .count();
        let edit_count = actions
            .iter()
            .filter(|a| a.action == actions::EDITS_PUBLISHED.to_vec())
            .count();

        // 18 spaces: 11 canonical + 7 non-canonical
        assert_eq!(space_count, 18);
        // 14 explicit edges + 5 topic edges = 19 trust extensions
        assert_eq!(trust_count, 19);
        // 6 edits
        assert_eq!(edit_count, 6);
    }
}
