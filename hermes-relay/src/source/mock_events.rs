//! Mock event builders mirroring contract action types.
//!
//! These builders create `Action` events in the chain format, matching
//! the action types from the Space Registry contract:
//!
//! - `space_created` → `GOVERNANCE.SPACE_ID_REGISTERED` action
//! - `subspace_added` → `GOVERNANCE.SUBSPACE_ADDED` action
//! - `subspace_verified` → `GOVERNANCE.SUBSPACE_VERIFIED` action
//! - `subspace_related` → `GOVERNANCE.SUBSPACE_RELATED` action
//! - `subspace_topic_declared` → `GOVERNANCE.SUBSPACE_TOPIC_DECLARED` action
//! - `edit_published` → `GOVERNANCE.EDITS_PUBLISHED` action
//! - And more...
//!
//! # Example
//!
//! ```ignore
//! use hermes_relay::source::events;
//!
//! let actions = vec![
//!     // Create a personal space
//!     events::space_created([0x01; 16], [0xaa; 32]),
//!     // Add a verified subspace
//!     events::subspace_verified([0x01; 16], [0x02; 16]),
//!     // Publish edits with IPFS hash
//!     events::edit_published([0x01; 16], "QmYwAPJzv5CZsnANOTaREALhashhere"),
//! ];
//! ```

use crate::actions;
use hermes_substream::pb::hermes::Action;

// =============================================================================
// Type aliases
// =============================================================================

pub type SpaceId = [u8; 16];
pub type TopicId = [u8; 16];
pub type Address = [u8; 32];
pub type ProposalId = [u8; 32];

// =============================================================================
// Space Registration Actions
// =============================================================================

/// Create a SPACE_REGISTERED action (personal space).
///
/// - `space_id`: The 16-byte ID of the new space
/// - `owner`: The 32-byte owner address (stored in topic field as bytes32(bytes20(address)))
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

/// Create a SPACE_MIGRATED action.
///
/// - `space_id`: The 16-byte ID of the space being migrated
/// - `new_space_address`: The new contract address (as bytes32(bytes20(address)))
pub fn space_migrated(space_id: SpaceId, new_space_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SPACE_MIGRATED.to_vec(),
        topic: new_space_address.to_vec(),
        data: vec![],
    }
}

// =============================================================================
// Subspace Actions
// =============================================================================

/// Create a SUBSPACE_ADDED action.
///
/// - `parent_space_id`: The parent space adding the subspace
/// - `subspace_id`: The subspace being added
pub fn subspace_added(parent_space_id: SpaceId, subspace_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&subspace_id);

    Action {
        from_id: parent_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_ADDED.to_vec(),
        topic,
        data: vec![],
    }
}

/// Create a SUBSPACE_REMOVED action.
///
/// - `parent_space_id`: The parent space removing the subspace
/// - `subspace_id`: The subspace being removed
pub fn subspace_removed(parent_space_id: SpaceId, subspace_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&subspace_id);

    Action {
        from_id: parent_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_REMOVED.to_vec(),
        topic,
        data: vec![],
    }
}

/// Create a SUBSPACE_VERIFIED action.
///
/// - `parent_space_id`: The space verifying the subspace
/// - `subspace_id`: The verified subspace
pub fn subspace_verified(parent_space_id: SpaceId, subspace_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&subspace_id);

    Action {
        from_id: parent_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_VERIFIED.to_vec(),
        topic,
        data: vec![],
    }
}

/// Create a SUBSPACE_RELATED action.
///
/// - `parent_space_id`: The space marking another as related
/// - `subspace_id`: The related subspace
pub fn subspace_related(parent_space_id: SpaceId, subspace_id: SpaceId) -> Action {
    let mut topic = vec![0u8; 16];
    topic.extend_from_slice(&subspace_id);

    Action {
        from_id: parent_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_RELATED.to_vec(),
        topic,
        data: vec![],
    }
}

/// Create a SUBSPACE_TOPIC_DECLARED action.
///
/// - `parent_space_id`: The parent space declaring the topic
/// - `subspace_id`: The subspace (first 16 bytes of topic field)
/// - `topic_id`: The topic ID (last 16 bytes of topic field)
pub fn subspace_topic_declared(
    parent_space_id: SpaceId,
    subspace_id: SpaceId,
    topic_id: TopicId,
) -> Action {
    let mut topic = subspace_id.to_vec();
    topic.extend_from_slice(&topic_id);

    Action {
        from_id: parent_space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::SUBSPACE_TOPIC_DECLARED.to_vec(),
        topic,
        data: vec![],
    }
}

// =============================================================================
// Proposal Actions
// =============================================================================

/// Voting mode for proposals (matches DAOSpace contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VotingMode {
    /// Fast path - threshold-based, immediate execution, single action only
    Fast = 0,
    /// Slow path - majority voting with voting window, multiple actions allowed
    Slow = 1,
}

/// Vote option for proposals (matches DAOSpace contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VoteOption {
    None = 0,
    Yes = 1,
    No = 2,
    Abstain = 3,
}

/// DAOSpace function selectors for proposal actions.
pub mod selectors {
    /// addMember(address)
    pub const ADD_MEMBER: [u8; 4] = [0xca, 0x6d, 0x56, 0xdc];
    /// removeMember(address)
    pub const REMOVE_MEMBER: [u8; 4] = [0x0b, 0x1c, 0xa4, 0x9a];
    /// addEditor(address)
    pub const ADD_EDITOR: [u8; 4] = [0xe5, 0x97, 0x5b, 0xdc];
    /// removeEditor(address)
    pub const REMOVE_EDITOR: [u8; 4] = [0x2d, 0x55, 0xfe, 0xaf];
    /// publish(bytes32,bytes,bytes)
    pub const PUBLISH: [u8; 4] = [0x6b, 0x47, 0xf6, 0x1a];
    /// flag(bytes32,bytes)
    pub const FLAG: [u8; 4] = [0xfe, 0x1e, 0x30, 0x42];
    /// unflag(bytes32,bytes)
    pub const UNFLAG: [u8; 4] = [0xc6, 0x96, 0x84, 0x0f];
}

/// A proposal action (to, value, data).
#[derive(Debug, Clone)]
pub struct ProposalAction {
    pub to: [u8; 20],
    pub value: [u8; 32],
    pub data: Vec<u8>,
}

impl ProposalAction {
    /// Create an addMember action.
    pub fn add_member(member_address: [u8; 20]) -> Self {
        let mut data = selectors::ADD_MEMBER.to_vec();
        // ABI-encode address (padded to 32 bytes)
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&member_address);
        Self {
            to: [0u8; 20], // Target is the DAOSpace contract itself
            value: [0u8; 32],
            data,
        }
    }

    /// Create an addEditor action.
    pub fn add_editor(editor_address: [u8; 20]) -> Self {
        let mut data = selectors::ADD_EDITOR.to_vec();
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&editor_address);
        Self {
            to: [0u8; 20],
            value: [0u8; 32],
            data,
        }
    }

    /// Create a publish action.
    pub fn publish(topic: [u8; 32], content_uri: &[u8], metadata: &[u8]) -> Self {
        let mut data = selectors::PUBLISH.to_vec();
        // Simplified encoding - just append the topic and content
        data.extend_from_slice(&topic);
        data.extend_from_slice(content_uri);
        data.extend_from_slice(metadata);
        Self {
            to: [0u8; 20],
            value: [0u8; 32],
            data,
        }
    }
}

/// ABI-encode proposal data: (VotingMode, Action[])
fn encode_proposal_data(voting_mode: VotingMode, actions: &[ProposalAction]) -> Vec<u8> {
    // Simplified ABI encoding for mock purposes
    // Real ABI encoding is more complex, but this is sufficient for testing
    let mut data = Vec::new();

    // VotingMode as uint8 padded to 32 bytes
    data.extend_from_slice(&[0u8; 31]);
    data.push(voting_mode as u8);

    // Offset to Action[] (64 bytes from start - after voting_mode and this offset)
    data.extend_from_slice(&[0u8; 31]);
    data.push(64);

    // Action[] length
    data.extend_from_slice(&[0u8; 31]);
    data.push(actions.len() as u8);

    // Each action (simplified - real encoding has dynamic offsets)
    for action in actions {
        // to (address padded to 32 bytes)
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&action.to);

        // value (uint256)
        data.extend_from_slice(&action.value);

        // data offset (we'll append data inline for simplicity)
        let data_offset = 96u64; // Fixed offset for mock
        data.extend_from_slice(&[0u8; 24]);
        data.extend_from_slice(&data_offset.to_be_bytes());

        // data length
        data.extend_from_slice(&[0u8; 31]);
        data.push(action.data.len() as u8);

        // data content (padded to 32 bytes)
        data.extend_from_slice(&action.data);
        let padding = (32 - (action.data.len() % 32)) % 32;
        data.extend_from_slice(&vec![0u8; padding]);
    }

    data
}

/// ABI-encode proposal vote data: (uint256 proposalId, VoteOption)
fn encode_vote_data(proposal_id: u64, vote_option: VoteOption) -> Vec<u8> {
    let mut data = Vec::new();

    // proposalId as uint256
    data.extend_from_slice(&[0u8; 24]);
    data.extend_from_slice(&proposal_id.to_be_bytes());

    // VoteOption as uint8 padded to 32 bytes
    data.extend_from_slice(&[0u8; 31]);
    data.push(vote_option as u8);

    data
}

/// Create a PROPOSAL_CREATED action.
///
/// - `space_id`: The space creating the proposal
/// - `proposal_id`: The proposal ID (32 bytes)
/// - `voting_mode`: Fast or Slow path
/// - `proposal_actions`: The actions to execute if proposal passes
pub fn proposal_created(
    space_id: SpaceId,
    proposal_id: ProposalId,
    voting_mode: VotingMode,
    proposal_actions: Vec<ProposalAction>,
) -> Action {
    let data = encode_proposal_data(voting_mode, &proposal_actions);

    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::PROPOSAL_CREATED.to_vec(),
        topic: proposal_id.to_vec(),
        data,
    }
}

/// Create a PROPOSAL_VOTED action.
///
/// - `voter_id`: The voter's space ID
/// - `space_id`: The space containing the proposal
/// - `proposal_id`: The proposal being voted on
/// - `vote_option`: The vote choice
pub fn proposal_voted(
    voter_id: SpaceId,
    space_id: SpaceId,
    proposal_id: ProposalId,
    vote_option: VoteOption,
) -> Action {
    // Extract proposal counter from proposal_id (last 8 bytes as u64)
    let proposal_counter = u64::from_be_bytes(proposal_id[24..32].try_into().unwrap_or([0; 8]));
    let data = encode_vote_data(proposal_counter, vote_option);

    Action {
        from_id: voter_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::PROPOSAL_VOTED.to_vec(),
        topic: proposal_id.to_vec(),
        data,
    }
}

/// Create a PROPOSAL_EXECUTED action.
///
/// - `space_id`: The space with the executed proposal
/// - `proposal_id`: The executed proposal ID
pub fn proposal_executed(space_id: SpaceId, proposal_id: ProposalId) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::PROPOSAL_EXECUTED.to_vec(),
        topic: proposal_id.to_vec(),
        data: vec![],
    }
}

// =============================================================================
// Editor/Member Actions
// =============================================================================

/// Create an EDITOR_ADDED action.
///
/// - `space_id`: The space adding the editor
/// - `editor_address`: The editor's address (as bytes32(bytes20(address)))
pub fn editor_added(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::EDITOR_ADDED.to_vec(),
        topic: editor_address.to_vec(),
        data: vec![],
    }
}

/// Create an EDITOR_REMOVED action.
///
/// - `space_id`: The space removing the editor
/// - `editor_address`: The editor's address (as bytes32(bytes20(address)))
pub fn editor_removed(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::EDITOR_REMOVED.to_vec(),
        topic: editor_address.to_vec(),
        data: vec![],
    }
}

/// Create a MEMBER_ADDED action.
///
/// - `space_id`: The space adding the member
/// - `member_address`: The member's address (as bytes32(bytes20(address)))
pub fn member_added(space_id: SpaceId, member_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::MEMBER_ADDED.to_vec(),
        topic: member_address.to_vec(),
        data: vec![],
    }
}

/// Create a MEMBER_REMOVED action.
///
/// - `space_id`: The space removing the member
/// - `member_address`: The member's address (as bytes32(bytes20(address)))
pub fn member_removed(space_id: SpaceId, member_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::MEMBER_REMOVED.to_vec(),
        topic: member_address.to_vec(),
        data: vec![],
    }
}

/// Create an EDITOR_FLAGGED action.
///
/// - `space_id`: The space flagging the editor
/// - `editor_address`: The flagged editor's address
pub fn editor_flagged(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::EDITOR_FLAGGED.to_vec(),
        topic: editor_address.to_vec(),
        data: vec![],
    }
}

/// Create an EDITOR_UNFLAGGED action.
///
/// - `space_id`: The space unflagging the editor
/// - `editor_address`: The unflagged editor's address
pub fn editor_unflagged(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::EDITOR_UNFLAGGED.to_vec(),
        topic: editor_address.to_vec(),
        data: vec![],
    }
}

/// Create a SPACE_LEFT action.
///
/// - `member_id`: The member leaving the space
/// - `space_id`: The space being left
/// - `role`: The role being left (e.g., keccak256("EDITOR") or keccak256("MEMBER"))
pub fn space_left(member_id: SpaceId, space_id: SpaceId, role: [u8; 32]) -> Action {
    Action {
        from_id: member_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::SPACE_LEFT.to_vec(),
        topic: role.to_vec(),
        data: vec![],
    }
}

// =============================================================================
// Content Actions
// =============================================================================

/// Create a TOPIC_DECLARED action.
///
/// - `space_id`: The space declaring the topic
/// - `topic_id`: The topic ID (keccak256 of topic name)
/// - `content_metadata`: Optional metadata
pub fn topic_declared(space_id: SpaceId, topic_id: [u8; 32], content_metadata: &[u8]) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::TOPIC_DECLARED.to_vec(),
        topic: topic_id.to_vec(),
        data: content_metadata.to_vec(),
    }
}

/// Create an EDITS_PUBLISHED action.
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

/// Create a FLAGGED action.
///
/// - `flagger_id`: The space flagging the content
/// - `space_id`: The space being flagged
/// - `flagged_uri`: URI of the flagged content
pub fn flagged(flagger_id: SpaceId, space_id: SpaceId, flagged_uri: &str) -> Action {
    Action {
        from_id: flagger_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::FLAGGED.to_vec(),
        topic: vec![0u8; 32], // Optional topic
        data: flagged_uri.as_bytes().to_vec(),
    }
}

/// Create an UNFLAGGED action.
///
/// - `unflagger_id`: The space unflagging the content
/// - `space_id`: The space being unflagged
/// - `unflagged_uri`: URI of the unflagged content
pub fn unflagged(unflagger_id: SpaceId, space_id: SpaceId, unflagged_uri: &str) -> Action {
    Action {
        from_id: unflagger_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::UNFLAGGED.to_vec(),
        topic: vec![0u8; 32], // Optional topic
        data: unflagged_uri.as_bytes().to_vec(),
    }
}

// =============================================================================
// Permissionless Voting Actions
// =============================================================================

/// Create an UPVOTED action.
///
/// - `voter_id`: The voter's space ID
/// - `object_type`: 4-byte object type identifier
/// - `object_id`: 16-byte object ID
/// - `version`: Version number
/// - `group_id`: Group ID for the vote
/// - `space_pov`: Space point-of-view
pub fn upvoted(
    voter_id: SpaceId,
    object_type: [u8; 4],
    object_id: SpaceId,
    version: u16,
    group_id: SpaceId,
    space_pov: SpaceId,
) -> Action {
    // Topic: bytes32(bytes4(objectType) << 224) | (bytes16(objectId) << 96)
    let mut topic = vec![0u8; 32];
    topic[0..4].copy_from_slice(&object_type);
    topic[4..20].copy_from_slice(&object_id);

    // Data: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
    let mut data = Vec::new();
    data.extend_from_slice(&version.to_be_bytes());
    data.extend_from_slice(&group_id);
    data.extend_from_slice(&space_pov);

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::UPVOTED.to_vec(),
        topic,
        data,
    }
}

/// Create a DOWNVOTED action.
///
/// - `voter_id`: The voter's space ID
/// - `object_type`: 4-byte object type identifier
/// - `object_id`: 16-byte object ID
/// - `version`: Version number
/// - `group_id`: Group ID for the vote
/// - `space_pov`: Space point-of-view
pub fn downvoted(
    voter_id: SpaceId,
    object_type: [u8; 4],
    object_id: SpaceId,
    version: u16,
    group_id: SpaceId,
    space_pov: SpaceId,
) -> Action {
    let mut topic = vec![0u8; 32];
    topic[0..4].copy_from_slice(&object_type);
    topic[4..20].copy_from_slice(&object_id);

    let mut data = Vec::new();
    data.extend_from_slice(&version.to_be_bytes());
    data.extend_from_slice(&group_id);
    data.extend_from_slice(&space_pov);

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::DOWNVOTED.to_vec(),
        topic,
        data,
    }
}

/// Create an UNVOTED action.
///
/// - `voter_id`: The voter's space ID
/// - `object_type`: 4-byte object type identifier
/// - `object_id`: 16-byte object ID
/// - `version`: Version number
/// - `group_id`: Group ID for the vote
/// - `space_pov`: Space point-of-view
pub fn unvoted(
    voter_id: SpaceId,
    object_type: [u8; 4],
    object_id: SpaceId,
    version: u16,
    group_id: SpaceId,
    space_pov: SpaceId,
) -> Action {
    let mut topic = vec![0u8; 32];
    topic[0..4].copy_from_slice(&object_type);
    topic[4..20].copy_from_slice(&object_id);

    let mut data = Vec::new();
    data.extend_from_slice(&version.to_be_bytes());
    data.extend_from_slice(&group_id);
    data.extend_from_slice(&space_pov);

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::UNVOTED.to_vec(),
        topic,
        data,
    }
}

// =============================================================================
// Helper functions
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

/// Helper to create a proposal ID from a single byte.
pub const fn make_proposal_id(last_byte: u8) -> ProposalId {
    make_address(last_byte)
}

// =============================================================================
// Test topology - comprehensive example with all action types
// =============================================================================

/// Well-known space IDs for testing
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

    // Proposal IDs
    pub const PROPOSAL_1: ProposalId = make_proposal_id(0xA1);
    pub const PROPOSAL_2: ProposalId = make_proposal_id(0xA2);

    // Object types for voting
    pub const OBJECT_TYPE_ENTITY: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
    pub const OBJECT_TYPE_TRIPLE: [u8; 4] = [0x00, 0x00, 0x00, 0x02];

    /// Generate a comprehensive set of test events covering all action types.
    ///
    /// Returns actions for:
    /// - Space registrations (personal and DAO)
    /// - Subspace operations (added, removed, verified, related, topic declared)
    /// - Editor/member management
    /// - Proposals (created, voted, executed)
    /// - Content operations (edits, flagging)
    /// - Permissionless voting
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

        // Phase 2: Subspace operations - verified
        actions.push(subspace_verified(ROOT_SPACE_ID, SPACE_A));
        actions.push(subspace_verified(ROOT_SPACE_ID, SPACE_B));
        actions.push(subspace_verified(SPACE_A, SPACE_C));
        actions.push(subspace_verified(SPACE_B, SPACE_E));
        actions.push(subspace_verified(SPACE_C, SPACE_F));
        actions.push(subspace_verified(SPACE_H, SPACE_I));
        actions.push(subspace_verified(SPACE_H, SPACE_J));
        actions.push(subspace_verified(SPACE_X, SPACE_Y));
        actions.push(subspace_verified(SPACE_Y, SPACE_Z));
        actions.push(subspace_verified(SPACE_P, SPACE_Q));

        // Phase 3: Subspace operations - related
        actions.push(subspace_related(ROOT_SPACE_ID, SPACE_H));
        actions.push(subspace_related(SPACE_A, SPACE_D));
        actions.push(subspace_related(SPACE_C, SPACE_G));
        actions.push(subspace_related(SPACE_X, SPACE_W));

        // Phase 4: Subspace topic declarations
        actions.push(subspace_topic_declared(SPACE_B, SPACE_H, TOPIC_H));
        actions.push(subspace_topic_declared(ROOT_SPACE_ID, SPACE_E, TOPIC_E));
        actions.push(subspace_topic_declared(SPACE_A, SPACE_A, TOPIC_SHARED));
        actions.push(subspace_topic_declared(SPACE_X, SPACE_A, TOPIC_A));
        actions.push(subspace_topic_declared(SPACE_P, SPACE_Q, TOPIC_Q));

        // Phase 5: Editor/member operations
        actions.push(editor_added(SPACE_A, USER_2));
        actions.push(member_added(SPACE_A, USER_3));
        actions.push(editor_flagged(SPACE_B, USER_1));
        actions.push(editor_unflagged(SPACE_B, USER_1));

        // Phase 6: Proposals (fast path add member proposal)
        actions.push(proposal_created(
            SPACE_A,
            PROPOSAL_1,
            VotingMode::Fast,
            vec![ProposalAction::add_member([0x11; 20])],
        ));
        actions.push(proposal_voted(
            SPACE_B,
            SPACE_A,
            PROPOSAL_1,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_C,
            SPACE_A,
            PROPOSAL_1,
            VoteOption::Yes,
        ));
        actions.push(proposal_executed(SPACE_A, PROPOSAL_1));

        // Phase 7: Edits
        actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit1CreatePersons"));
        actions.push(edit_published(ROOT_SPACE_ID, "QmRootEdit2AddDescriptions"));
        actions.push(edit_published(SPACE_A, "QmSpaceAEdit1CreateOrg"));
        actions.push(edit_published(SPACE_A, "QmSpaceAEdit2CreateRelations"));
        actions.push(edit_published(SPACE_B, "QmSpaceBEdit1CreateDoc"));
        actions.push(edit_published(SPACE_C, "QmSpaceCEdit1CreateTopic"));

        // Phase 8: Content flagging
        actions.push(flagged(SPACE_A, SPACE_X, "ipfs://QmFlaggedContent1"));
        actions.push(unflagged(SPACE_A, SPACE_X, "ipfs://QmFlaggedContent1"));

        // Phase 9: Permissionless voting
        let entity_id = make_id(0xE1);
        actions.push(upvoted(
            SPACE_A,
            OBJECT_TYPE_ENTITY,
            entity_id,
            1,
            ROOT_SPACE_ID,
            ROOT_SPACE_ID,
        ));
        actions.push(downvoted(
            SPACE_B,
            OBJECT_TYPE_ENTITY,
            entity_id,
            1,
            ROOT_SPACE_ID,
            ROOT_SPACE_ID,
        ));
        actions.push(unvoted(
            SPACE_B,
            OBJECT_TYPE_ENTITY,
            entity_id,
            1,
            ROOT_SPACE_ID,
            ROOT_SPACE_ID,
        ));

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
    fn test_subspace_verified_format() {
        let parent = make_id(0x01);
        let subspace = make_id(0x02);
        let action = subspace_verified(parent, subspace);

        assert_eq!(action.from_id, parent.to_vec());
        assert_eq!(action.action, actions::SUBSPACE_VERIFIED.to_vec());
        assert_eq!(&action.topic[16..32], &subspace);
    }

    #[test]
    fn test_subspace_related_format() {
        let parent = make_id(0x01);
        let related = make_id(0x02);
        let action = subspace_related(parent, related);

        assert_eq!(action.from_id, parent.to_vec());
        assert_eq!(action.action, actions::SUBSPACE_RELATED.to_vec());
        assert_eq!(&action.topic[16..32], &related);
    }

    #[test]
    fn test_subspace_topic_declared_format() {
        let parent = make_id(0x01);
        let subspace = make_id(0x02);
        let topic = make_id(0x03);
        let action = subspace_topic_declared(parent, subspace, topic);

        assert_eq!(action.from_id, parent.to_vec());
        assert_eq!(action.action, actions::SUBSPACE_TOPIC_DECLARED.to_vec());
        assert_eq!(&action.topic[0..16], &subspace);
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
    fn test_flagged_format() {
        let flagger = make_id(0x01);
        let target = make_id(0x02);
        let action = flagged(flagger, target, "ipfs://test");

        assert_eq!(action.from_id, flagger.to_vec());
        assert_eq!(action.to_id, target.to_vec());
        assert_eq!(action.action, actions::FLAGGED.to_vec());
    }

    #[test]
    fn test_upvoted_format() {
        let voter = make_id(0x01);
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let object_id = make_id(0x02);
        let action = upvoted(
            voter,
            object_type,
            object_id,
            1,
            make_id(0x03),
            make_id(0x04),
        );

        assert_eq!(action.from_id, voter.to_vec());
        assert_eq!(action.action, actions::UPVOTED.to_vec());
        assert_eq!(&action.topic[0..4], &object_type);
        assert_eq!(&action.topic[4..20], &object_id);
    }

    #[test]
    fn test_topology_generate_counts() {
        let actions = test_topology::generate();

        let space_count = actions
            .iter()
            .filter(|a| a.action == actions::SPACE_REGISTERED.to_vec())
            .count();
        let verified_count = actions
            .iter()
            .filter(|a| a.action == actions::SUBSPACE_VERIFIED.to_vec())
            .count();
        let related_count = actions
            .iter()
            .filter(|a| a.action == actions::SUBSPACE_RELATED.to_vec())
            .count();
        let topic_declared_count = actions
            .iter()
            .filter(|a| a.action == actions::SUBSPACE_TOPIC_DECLARED.to_vec())
            .count();
        let edit_count = actions
            .iter()
            .filter(|a| a.action == actions::EDITS_PUBLISHED.to_vec())
            .count();

        // 18 spaces: 11 canonical + 7 non-canonical
        assert_eq!(space_count, 18);
        // 10 verified subspaces
        assert_eq!(verified_count, 10);
        // 4 related subspaces
        assert_eq!(related_count, 4);
        // 5 topic declarations
        assert_eq!(topic_declared_count, 5);
        // 6 edits
        assert_eq!(edit_count, 6);
    }

    #[test]
    fn test_proposal_created_format() {
        let space_id = make_id(0x01);
        let proposal_id = make_proposal_id(0xA1);
        let member_address = [0xBB; 20];

        let action = proposal_created(
            space_id,
            proposal_id,
            VotingMode::Fast,
            vec![ProposalAction::add_member(member_address)],
        );

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.action, actions::PROPOSAL_CREATED.to_vec());
        assert_eq!(action.topic, proposal_id.to_vec());
        // Data should not be empty - it contains encoded (VotingMode, Action[])
        assert!(!action.data.is_empty());
        // First 32 bytes should be VotingMode (0 = Fast, padded)
        assert_eq!(action.data[31], 0); // VotingMode::Fast
    }

    #[test]
    fn test_proposal_voted_format() {
        let voter_id = make_id(0x01);
        let space_id = make_id(0x02);
        let proposal_id = make_proposal_id(0xA1);

        let action = proposal_voted(voter_id, space_id, proposal_id, VoteOption::Yes);

        assert_eq!(action.from_id, voter_id.to_vec());
        assert_eq!(action.to_id, space_id.to_vec());
        assert_eq!(action.action, actions::PROPOSAL_VOTED.to_vec());
        assert_eq!(action.topic, proposal_id.to_vec());
        // Data should contain encoded (uint256 proposalId, VoteOption)
        assert_eq!(action.data.len(), 64); // Two 32-byte values
        assert_eq!(action.data[63], 1); // VoteOption::Yes
    }

    #[test]
    fn test_proposal_action_add_member() {
        let member_address = [0xAA; 20];
        let action = ProposalAction::add_member(member_address);

        // Calldata should start with ADD_MEMBER selector
        assert_eq!(&action.data[0..4], &selectors::ADD_MEMBER);
        // Followed by padded address
        assert_eq!(&action.data[4..16], &[0u8; 12]);
        assert_eq!(&action.data[16..36], &member_address);
    }
}
