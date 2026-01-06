//! Mock event builders mirroring contract action types.
//!
//! These builders create `Action` events in the chain format, matching
//! the action types from the Space Registry contract:
//!
//! - `personal_space_registered` → `SPACE_ID_REGISTERED` + `SPACE_TYPE_DECLARED` actions
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
//! let mut actions = Vec::new();
//! // Create a personal space (returns 2 events)
//! actions.extend(events::personal_space_registered([0x01; 16], [0xaa; 32]));
//! // Add a verified subspace
//! actions.push(events::subspace_verified([0x01; 16], [0x02; 16]));
//! // Publish edits with IPFS hash
//! actions.push(events::edit_published([0x01; 16], "QmYwAPJzv5CZsnANOTaREALhashhere"));
//! ```

use alloy::primitives::U256;
use alloy::sol;
use alloy::sol_types::SolType;

use crate::actions;
use hermes_substream::pb::hermes::Action;

// =============================================================================
// Type aliases
// =============================================================================

pub type SpaceId = [u8; 16];
pub type TopicId = [u8; 16];
pub type Address = [u8; 32];
pub type ProposalId = [u8; 16];

// =============================================================================
// Space Registration Actions
// =============================================================================

/// Create a SPACE_ID_REGISTERED action (just the registration, no type).
///
/// This is the base event emitted when any space is registered.
/// The space type is determined by a separate SPACE_TYPE_DECLARED event.
///
/// - `space_id`: The 16-byte ID of the new space
/// - `registrar`: The registrar's address as bytes32(bytes20(address))
///   For EOA spaces: the owner's address
///   For DAO spaces: the DAOSpace contract address
pub fn space_id_registered(space_id: SpaceId, registrar: Address) -> Action {
    Action {
        from_id: vec![0u8; 16],   // zeros per new contract
        to_id: space_id.to_vec(), // space_id goes here now
        action: actions::SPACE_REGISTERED.to_vec(),
        topic: registrar.to_vec(), // bytes32(bytes20(msg.sender))
        data: vec![],
    }
}

/// Create a SPACE_TYPE_DECLARED action.
///
/// - `space_id`: The space declaring its type
/// - `space_type`: The 32-byte space type hash (e.g., SPACE_TYPE_EOA or SPACE_TYPE_DAO)
/// - `version`: The version bytes (typically empty or version info)
pub fn space_type_declared(space_id: SpaceId, space_type: [u8; 32], version: &[u8]) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace
        action: actions::SPACE_TYPE_DECLARED.to_vec(),
        topic: space_type.to_vec(),
        data: version.to_vec(),
    }
}

/// Create all events for a personal (EOA) space registration.
///
/// Returns events in correct order:
/// 1. SPACE_ID_REGISTERED (from_id=zeros, to_id=space_id, topic=owner)
/// 2. SPACE_TYPE_DECLARED (type=EOA_SPACE)
///
/// - `space_id`: The 16-byte space ID
/// - `owner`: The owner's address as bytes32(bytes20(address))
pub fn personal_space_registered(space_id: SpaceId, owner: Address) -> Vec<Action> {
    vec![
        space_id_registered(space_id, owner),
        space_type_declared(space_id, actions::SPACE_TYPE_EOA, &[]),
    ]
}

/// Create all events for DAO space initialization.
///
/// Returns events in correct order:
/// 1. SPACE_ID_REGISTERED (from_id=zeros, to_id=space_id, topic=dao_address)
/// 2. SPACE_TYPE_DECLARED (type=DAO_SPACE, data=version)
/// 3. EDITS_PUBLISHED (if initial_edits is Some)
/// 4. EDITOR_ADDED for each initial editor
/// 5. MEMBER_ADDED for each initial member
///
/// - `space_id`: The 16-byte space ID
/// - `dao_address`: The DAOSpace contract address as bytes32(bytes20(address))
/// - `initial_edits`: Optional IPFS hash for initial edits to publish
/// - `initial_editors`: List of initial editor addresses (bytes20)
/// - `initial_members`: List of initial member addresses (bytes20)
pub fn dao_space_initialized(
    space_id: SpaceId,
    dao_address: Address,
    initial_edits: Option<&str>,
    initial_editors: &[[u8; 20]],
    initial_members: &[[u8; 20]],
) -> Vec<Action> {
    let mut actions = vec![
        space_id_registered(space_id, dao_address),
        space_type_declared(space_id, actions::SPACE_TYPE_DAO, b"1.0.0"),
    ];

    // Add EDITS_PUBLISHED if initial edits provided
    if let Some(edits_hash) = initial_edits {
        actions.push(edit_published(space_id, edits_hash));
    }

    // Add EDITOR_ADDED for each initial editor
    for editor in initial_editors {
        let mut address = [0u8; 32];
        address[12..32].copy_from_slice(editor);
        actions.push(editor_added(space_id, address));
    }

    // Add MEMBER_ADDED for each initial member
    for member in initial_members {
        let mut address = [0u8; 32];
        address[12..32].copy_from_slice(member);
        actions.push(member_added(space_id, address));
    }

    actions
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

    /// Create a removeMember action.
    pub fn remove_member(member_address: [u8; 20]) -> Self {
        let mut data = selectors::REMOVE_MEMBER.to_vec();
        // ABI-encode address (padded to 32 bytes)
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&member_address);
        Self {
            to: [0u8; 20],
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

    /// Create a removeEditor action.
    pub fn remove_editor(editor_address: [u8; 20]) -> Self {
        let mut data = selectors::REMOVE_EDITOR.to_vec();
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

    /// Create a flag action.
    pub fn flag(target: [u8; 32], content_uri: &[u8]) -> Self {
        // ABI-encode (bytes32, bytes)
        use alloy::primitives::{Bytes, FixedBytes};
        type FlagArgsType = sol! { (bytes32, bytes) };
        let encoded = FlagArgsType::abi_encode(&(
            FixedBytes::<32>::from(target),
            Bytes::copy_from_slice(content_uri),
        ));

        let mut data = selectors::FLAG.to_vec();
        data.extend_from_slice(&encoded);
        Self {
            to: [0u8; 20],
            value: [0u8; 32],
            data,
        }
    }

    /// Create an unflag action.
    pub fn unflag(target: [u8; 32], content_uri: &[u8]) -> Self {
        // ABI-encode (bytes32, bytes)
        use alloy::primitives::{Bytes, FixedBytes};
        type UnflagArgsType = sol! { (bytes32, bytes) };
        let encoded = UnflagArgsType::abi_encode(&(
            FixedBytes::<32>::from(target),
            Bytes::copy_from_slice(content_uri),
        ));

        let mut data = selectors::UNFLAG.to_vec();
        data.extend_from_slice(&encoded);
        Self {
            to: [0u8; 20],
            value: [0u8; 32],
            data,
        }
    }
}

// Solidity type for proper ABI encoding
sol! {
    struct SolAction {
        address to;
        uint256 value;
        bytes data;
    }
}

// Type alias for encoding proposal data: (bytes16, uint8, Action[])
type ProposalDataType = sol! { (bytes16, uint8, SolAction[]) };

/// ABI-encode proposal data: (bytes16 proposalId, uint8 votingMode, Action[])
fn encode_proposal_data(
    proposal_id: ProposalId,
    voting_mode: VotingMode,
    actions: &[ProposalAction],
) -> Vec<u8> {
    use alloy::primitives::FixedBytes;
    // Convert to Solidity types
    let sol_actions: Vec<SolAction> = actions
        .iter()
        .map(|a| SolAction {
            to: alloy::primitives::Address::from_slice(&a.to),
            value: U256::from_be_slice(&a.value),
            data: a.data.clone().into(),
        })
        .collect();

    ProposalDataType::abi_encode(&(
        FixedBytes::<16>::from_slice(&proposal_id),
        voting_mode as u8,
        sol_actions,
    ))
}

// Type alias for encoding vote data: (bytes16, uint8)
type VoteDataType = sol! { (bytes16, uint8) };

/// ABI-encode proposal vote data: (bytes16 proposalId, uint8 voteOption)
fn encode_vote_data(proposal_id: ProposalId, vote_option: VoteOption) -> Vec<u8> {
    use alloy::primitives::FixedBytes;
    VoteDataType::abi_encode(&(
        FixedBytes::<16>::from_slice(&proposal_id),
        vote_option as u8,
    ))
}

// Solidity type for permissionless vote data: (uint16 version, bytes16 groupId, bytes16 spacePOV)
type PermissionlessVoteDataType = sol! { (uint16, bytes16, bytes16) };

/// ABI-encode permissionless vote data: (uint16 version, bytes16 groupId, bytes16 spacePOV)
fn encode_permissionless_vote_data(version: u16, group_id: SpaceId, space_pov: SpaceId) -> Vec<u8> {
    use alloy::primitives::FixedBytes;
    PermissionlessVoteDataType::abi_encode(&(
        version,
        FixedBytes::<16>::from_slice(&group_id),
        FixedBytes::<16>::from_slice(&space_pov),
    ))
}

// Solidity type for content URI: bytes
type ContentUriType = sol! { bytes };

/// ABI-encode content URI for flagged/unflagged: abi.encode(bytes(uri))
fn encode_content_uri(uri: &str) -> Vec<u8> {
    ContentUriType::abi_encode(&uri.as_bytes().to_vec())
}

/// Create a PROPOSAL_CREATED action.
///
/// - `space_id`: The space creating the proposal
/// - `proposal_id`: The proposal ID (16 bytes)
/// - `voting_mode`: Fast or Slow path
/// - `proposal_actions`: The actions to execute if proposal passes
pub fn proposal_created(
    space_id: SpaceId,
    proposal_id: ProposalId,
    voting_mode: VotingMode,
    proposal_actions: Vec<ProposalAction>,
) -> Action {
    let data = encode_proposal_data(proposal_id, voting_mode, &proposal_actions);

    // Pad proposal_id to 32 bytes for topic field
    let mut topic = [0u8; 32];
    topic[..16].copy_from_slice(&proposal_id);

    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::PROPOSAL_CREATED.to_vec(),
        topic: topic.to_vec(),
        data,
    }
}

/// Create a PROPOSAL_SETTINGS_SELECTED action.
///
/// This event is emitted after PROPOSAL_CREATED to convey the proposal settings.
/// The pipeline squashes PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED into a single event.
///
/// - `space_id`: The space with the proposal
/// - `proposal_id`: The proposal ID (16 bytes)
/// - `voting_delay`: Voting delay in seconds
/// - `voting_period`: Voting period in seconds
/// - `quorum`: Quorum percentage
/// - `threshold`: Approval threshold percentage
pub fn proposal_settings_used(
    space_id: SpaceId,
    proposal_id: ProposalId,
    voting_delay: u32,
    voting_period: u32,
    quorum: u16,
    threshold: u16,
) -> Action {
    // Encode settings as: (uint32 votingDelay, uint32 votingPeriod, uint16 quorum, uint16 threshold)
    use alloy::primitives::FixedBytes;
    use alloy::sol_types::SolType;

    type SettingsType = sol! { (bytes16, uint32, uint32, uint16, uint16) };
    let data = SettingsType::abi_encode(&(
        FixedBytes::<16>::from_slice(&proposal_id),
        voting_delay,
        voting_period,
        quorum,
        threshold,
    ));

    // Pad proposal_id to 32 bytes for topic field
    let mut topic = [0u8; 32];
    topic[..16].copy_from_slice(&proposal_id);

    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::PROPOSAL_SETTINGS_SELECTED.to_vec(),
        topic: topic.to_vec(),
        data,
    }
}

/// Create a PROPOSAL_VOTED action.
///
/// - `voter_id`: The voter's space ID
/// - `space_id`: The space containing the proposal
/// - `proposal_id`: The proposal being voted on (16 bytes)
/// - `vote_option`: The vote choice
pub fn proposal_voted(
    voter_id: SpaceId,
    space_id: SpaceId,
    proposal_id: ProposalId,
    vote_option: VoteOption,
) -> Action {
    let data = encode_vote_data(proposal_id, vote_option);

    // Pad proposal_id to 32 bytes for topic field
    let mut topic = [0u8; 32];
    topic[..16].copy_from_slice(&proposal_id);

    Action {
        from_id: voter_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::PROPOSAL_VOTED.to_vec(),
        topic: topic.to_vec(),
        data,
    }
}

/// Create a PROPOSAL_EXECUTED action.
///
/// - `space_id`: The space with the executed proposal
/// - `proposal_id`: The executed proposal ID (16 bytes)
pub fn proposal_executed(space_id: SpaceId, proposal_id: ProposalId) -> Action {
    // Pad proposal_id to 32 bytes for topic field
    let mut topic = [0u8; 32];
    topic[..16].copy_from_slice(&proposal_id);

    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::PROPOSAL_EXECUTED.to_vec(),
        topic: topic.to_vec(),
        data: vec![],
    }
}

// =============================================================================
// Editor/Member Actions
// =============================================================================

/// Create an EDITOR_ADDED action.
///
/// ZC16 format:
/// - `space_id`: The space adding the editor
/// - `editor_address`: The editor's address (as bytes32(bytes20(address)))
///   - topic: bytes32(spaceId) - target space ID
///   - data: abi.encode(address) - 32 bytes with address
pub fn editor_added(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::EDITOR_ADDED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (can be zeros for self)
        data: editor_address.to_vec(), // ZC16: address is ABI-encoded in data field
    }
}

/// Create an EDITOR_REMOVED action.
///
/// ZC16 format:
/// - `space_id`: The space removing the editor
/// - `editor_address`: The editor's address (as bytes32(bytes20(address)))
///   - topic: bytes32(spaceId) - target space ID
///   - data: abi.encode(address) - 32 bytes with address
pub fn editor_removed(space_id: SpaceId, editor_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::EDITOR_REMOVED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (can be zeros for self)
        data: editor_address.to_vec(), // ZC16: address is ABI-encoded in data field
    }
}

/// Create a MEMBER_ADDED action.
///
/// ZC16 format:
/// - `space_id`: The space adding the member
/// - `member_address`: The member's address (as bytes32(bytes20(address)))
///   - topic: bytes32(spaceId) - target space ID
///   - data: abi.encode(address) - 32 bytes with address
pub fn member_added(space_id: SpaceId, member_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::MEMBER_ADDED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (can be zeros for self)
        data: member_address.to_vec(), // ZC16: address is ABI-encoded in data field
    }
}

/// Create a MEMBER_REMOVED action.
///
/// ZC16 format:
/// - `space_id`: The space removing the member
/// - `member_address`: The member's address (as bytes32(bytes20(address)))
///   - topic: bytes32(spaceId) - target space ID
///   - data: abi.encode(address) - 32 bytes with address
pub fn member_removed(space_id: SpaceId, member_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::MEMBER_REMOVED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (can be zeros for self)
        data: member_address.to_vec(), // ZC16: address is ABI-encoded in data field
    }
}

/// Create a SPACE_FAST_PATH_RESTRICTED action.
///
/// - `space_id`: The space restricting another space from fast path
/// - `restricted_space_address`: The restricted space's address
pub fn space_fast_path_restricted(space_id: SpaceId, restricted_space_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::SPACE_FAST_PATH_RESTRICTED.to_vec(),
        topic: restricted_space_address.to_vec(),
        data: vec![],
    }
}

/// Create a SPACE_FAST_PATH_UNRESTRICTED action.
///
/// - `space_id`: The space unrestricting another space from fast path
/// - `unrestricted_space_address`: The unrestricted space's address
pub fn space_fast_path_unrestricted(space_id: SpaceId, unrestricted_space_address: Address) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: space_id.to_vec(), // from_id == to_id for DAOSpace actions
        action: actions::SPACE_FAST_PATH_UNRESTRICTED.to_vec(),
        topic: unrestricted_space_address.to_vec(),
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

// Solidity type for topic declared data: bytes16
type TopicDeclaredDataType = sol! { bytes16 };

/// Create a TOPIC_DECLARED action.
///
/// - `space_id`: The space declaring the topic
/// - `topic_id`: The 16-byte topic UUID
pub fn topic_declared(space_id: SpaceId, topic_id: TopicId) -> Action {
    use alloy::primitives::FixedBytes;
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::TOPIC_DECLARED.to_vec(),
        topic: vec![0u8; 32], // Empty topic field
        data: TopicDeclaredDataType::abi_encode(&FixedBytes::<16>::from_slice(&topic_id)),
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
        data: encode_content_uri(flagged_uri),
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
        data: encode_content_uri(unflagged_uri),
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

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::UPVOTED.to_vec(),
        topic,
        data: encode_permissionless_vote_data(version, group_id, space_pov),
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

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::DOWNVOTED.to_vec(),
        topic,
        data: encode_permissionless_vote_data(version, group_id, space_pov),
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

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![0u8; 16],
        action: actions::UNVOTED.to_vec(),
        topic,
        data: encode_permissionless_vote_data(version, group_id, space_pov),
    }
}

// =============================================================================
// Helper functions
// =============================================================================

/// Helper to create a well-known UUID v4 from a single byte.
///
/// Creates a valid RFC 4122 UUID v4 with:
/// - Version bits (byte 6 high nibble = 4)
/// - Variant bits (byte 8 high nibble = 8)
/// - Distinguishing value in last byte
///
/// Example: `make_id(0x0A)` produces a UUID like `00000000-0000-4000-8000-00000000000A`
pub const fn make_id(last_byte: u8) -> SpaceId {
    [0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, last_byte]
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
///
/// Uses the same UUID v4 format as make_id for consistency.
pub const fn make_proposal_id(last_byte: u8) -> ProposalId {
    make_id(last_byte)
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
    pub const PROPOSAL_3: ProposalId = make_proposal_id(0xA3);
    pub const PROPOSAL_4: ProposalId = make_proposal_id(0xA4);
    pub const PROPOSAL_5: ProposalId = make_proposal_id(0xA5);
    pub const PROPOSAL_6: ProposalId = make_proposal_id(0xA6);
    pub const PROPOSAL_7: ProposalId = make_proposal_id(0xA7);

    // Object types for voting
    pub const OBJECT_TYPE_ENTITY: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
    pub const OBJECT_TYPE_TRIPLE: [u8; 4] = [0x00, 0x00, 0x00, 0x02];

    // Editor address for initial DAO editors (USER_Q as bytes20)
    pub const EDITOR_Q: [u8; 20] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x31,
    ];

    // DAOSpace contract address for SPACE_P (as bytes32(bytes20(address)))
    pub const DAO_P_ADDRESS: Address = make_address(0xDA);

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

        // Phase 1: Create all spaces using new event sequence
        // Each personal_space_registered returns 2 events (SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED)
        actions.extend(personal_space_registered(ROOT_SPACE_ID, ROOT_OWNER));
        actions.extend(personal_space_registered(SPACE_A, USER_1));
        actions.extend(personal_space_registered(SPACE_B, USER_2));
        actions.extend(personal_space_registered(SPACE_C, USER_1));
        actions.extend(personal_space_registered(SPACE_D, USER_2));
        actions.extend(personal_space_registered(SPACE_E, USER_3));
        actions.extend(personal_space_registered(SPACE_F, USER_1));
        actions.extend(personal_space_registered(SPACE_G, USER_2));
        actions.extend(personal_space_registered(SPACE_H, USER_3));
        actions.extend(personal_space_registered(SPACE_I, USER_1));
        actions.extend(personal_space_registered(SPACE_J, USER_2));

        // Non-canonical - Island 1
        actions.extend(personal_space_registered(SPACE_X, USER_1));
        actions.extend(personal_space_registered(SPACE_Y, USER_2));
        actions.extend(personal_space_registered(SPACE_Z, USER_3));
        actions.extend(personal_space_registered(SPACE_W, USER_1));

        // Non-canonical - Island 2 (P is DAO with Q as initial editor)
        // Q must be created first as it's an initial editor
        actions.extend(personal_space_registered(SPACE_Q, USER_2));
        // DAO space P with SPACE_Q's address as initial editor
        actions.extend(dao_space_initialized(
            SPACE_P,
            DAO_P_ADDRESS,
            None,
            &[EDITOR_Q],
            &[],
        ));

        // Non-canonical - Island 3
        actions.extend(personal_space_registered(SPACE_S, USER_3));

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
        actions.push(space_fast_path_restricted(SPACE_B, USER_1));
        actions.push(space_fast_path_unrestricted(SPACE_B, USER_1));

        // Phase 6: Proposals with various actions

        // Proposal 1: Add member (PROPOSAL_CREATED + PROPOSAL_SETTINGS_SELECTED squashed by pipeline)
        actions.push(proposal_created(
            SPACE_A,
            PROPOSAL_1,
            VotingMode::Fast,
            vec![ProposalAction::add_member([0x11; 20])],
        ));
        actions.push(proposal_settings_used(
            SPACE_A, PROPOSAL_1, 0,     // voting_delay
            86400, // voting_period (1 day)
            50,    // quorum (50%)
            60,    // threshold (60%)
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
        let mut proposed_member_address = [0u8; 32];
        proposed_member_address[12..32].copy_from_slice(&[0x11; 20]);
        actions.push(member_added(SPACE_A, proposed_member_address));

        // Proposal 2: Remove member
        actions.push(proposal_created(
            SPACE_A,
            PROPOSAL_2,
            VotingMode::Slow,
            vec![ProposalAction::remove_member([0x12; 20])],
        ));
        actions.push(proposal_settings_used(
            SPACE_A, PROPOSAL_2, 3600,   // voting_delay (1 hour)
            172800, // voting_period (2 days)
            40,     // quorum (40%)
            70,     // threshold (70%)
        ));
        actions.push(proposal_voted(
            SPACE_B,
            SPACE_A,
            PROPOSAL_2,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_C,
            SPACE_A,
            PROPOSAL_2,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(SPACE_D, SPACE_A, PROPOSAL_2, VoteOption::No));
        actions.push(proposal_executed(SPACE_A, PROPOSAL_2));
        let mut removed_member_address = [0u8; 32];
        removed_member_address[12..32].copy_from_slice(&[0x12; 20]);
        actions.push(member_removed(SPACE_A, removed_member_address));

        // Proposal 3: Add editor
        actions.push(proposal_created(
            SPACE_B,
            PROPOSAL_3,
            VotingMode::Fast,
            vec![ProposalAction::add_editor([0x22; 20])],
        ));
        actions.push(proposal_settings_used(
            SPACE_B, PROPOSAL_3, 0,     // voting_delay
            86400, // voting_period (1 day)
            50,    // quorum (50%)
            60,    // threshold (60%)
        ));
        actions.push(proposal_voted(
            SPACE_A,
            SPACE_B,
            PROPOSAL_3,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_C,
            SPACE_B,
            PROPOSAL_3,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_E,
            SPACE_B,
            PROPOSAL_3,
            VoteOption::Abstain,
        ));
        actions.push(proposal_executed(SPACE_B, PROPOSAL_3));
        let mut new_editor_address = [0u8; 32];
        new_editor_address[12..32].copy_from_slice(&[0x22; 20]);
        actions.push(editor_added(SPACE_B, new_editor_address));

        // Proposal 4: Remove editor
        actions.push(proposal_created(
            SPACE_B,
            PROPOSAL_4,
            VotingMode::Slow,
            vec![ProposalAction::remove_editor([0x23; 20])],
        ));
        actions.push(proposal_settings_used(
            SPACE_B, PROPOSAL_4, 7200,   // voting_delay (2 hours)
            172800, // voting_period (2 days)
            45,     // quorum (45%)
            75,     // threshold (75%)
        ));
        actions.push(proposal_voted(
            SPACE_A,
            SPACE_B,
            PROPOSAL_4,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(SPACE_C, SPACE_B, PROPOSAL_4, VoteOption::No));
        actions.push(proposal_voted(
            SPACE_D,
            SPACE_B,
            PROPOSAL_4,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_F,
            SPACE_B,
            PROPOSAL_4,
            VoteOption::Yes,
        ));
        actions.push(proposal_executed(SPACE_B, PROPOSAL_4));
        let mut removed_editor_address = [0u8; 32];
        removed_editor_address[12..32].copy_from_slice(&[0x23; 20]);
        actions.push(editor_removed(SPACE_B, removed_editor_address));

        // Proposal 5: Flag content
        let mut flag_target = [0u8; 32];
        flag_target[..16].copy_from_slice(&make_id(0xF5));
        actions.push(proposal_created(
            SPACE_C,
            PROPOSAL_5,
            VotingMode::Fast,
            vec![ProposalAction::flag(
                flag_target,
                b"ipfs://QmFlaggedContent5",
            )],
        ));
        actions.push(proposal_settings_used(
            SPACE_C, PROPOSAL_5, 0,     // voting_delay
            86400, // voting_period (1 day)
            50,    // quorum (50%)
            60,    // threshold (60%)
        ));
        actions.push(proposal_voted(
            SPACE_A,
            SPACE_C,
            PROPOSAL_5,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_B,
            SPACE_C,
            PROPOSAL_5,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(SPACE_E, SPACE_C, PROPOSAL_5, VoteOption::No));
        actions.push(proposal_executed(SPACE_C, PROPOSAL_5));

        // Proposal 6: Unflag content
        let mut unflag_target = [0u8; 32];
        unflag_target[..16].copy_from_slice(&make_id(0xF6));
        actions.push(proposal_created(
            SPACE_C,
            PROPOSAL_6,
            VotingMode::Slow,
            vec![ProposalAction::unflag(
                unflag_target,
                b"ipfs://QmFlaggedContent6",
            )],
        ));
        actions.push(proposal_settings_used(
            SPACE_C, PROPOSAL_6, 5400,   // voting_delay (1.5 hours)
            172800, // voting_period (2 days)
            50,     // quorum (50%)
            65,     // threshold (65%)
        ));
        actions.push(proposal_voted(
            SPACE_A,
            SPACE_C,
            PROPOSAL_6,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_B,
            SPACE_C,
            PROPOSAL_6,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_D,
            SPACE_C,
            PROPOSAL_6,
            VoteOption::Abstain,
        ));
        actions.push(proposal_voted(SPACE_F, SPACE_C, PROPOSAL_6, VoteOption::No));
        actions.push(proposal_executed(SPACE_C, PROPOSAL_6));

        // Proposal 7: Publish edits (via proposal)
        let publish_topic = make_id(0x77);
        let mut publish_topic_array = [0u8; 32];
        publish_topic_array[..16].copy_from_slice(&publish_topic);
        actions.push(proposal_created(
            SPACE_B,
            PROPOSAL_7,
            VotingMode::Fast,
            vec![ProposalAction::publish(
                publish_topic_array,
                b"QmProposedEdits",
                b"version=1",
            )],
        ));
        actions.push(proposal_settings_used(
            SPACE_B, PROPOSAL_7, 0,     // voting_delay
            86400, // voting_period (1 day)
            50,    // quorum (50%)
            60,    // threshold (60%)
        ));
        actions.push(proposal_voted(
            SPACE_A,
            SPACE_B,
            PROPOSAL_7,
            VoteOption::Yes,
        ));
        actions.push(proposal_voted(
            SPACE_C,
            SPACE_B,
            PROPOSAL_7,
            VoteOption::Yes,
        ));
        actions.push(proposal_executed(SPACE_B, PROPOSAL_7));

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

        // Phase 10: Topic declarations (standalone, not trust-related)
        actions.push(topic_declared(ROOT_SPACE_ID, make_id(0xD1)));
        actions.push(topic_declared(SPACE_A, make_id(0xD2)));

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_id_registered_format() {
        let space_id = make_id(0x01);
        let owner = make_address(0xAA);
        let action = space_id_registered(space_id, owner);

        assert_eq!(action.from_id, vec![0u8; 16]); // zeros
        assert_eq!(action.to_id, space_id.to_vec());
        assert_eq!(action.action, actions::SPACE_REGISTERED.to_vec());
        assert_eq!(action.topic, owner.to_vec()); // registrar address
    }

    #[test]
    fn test_space_type_declared_format() {
        let space_id = make_id(0x01);
        let action = space_type_declared(space_id, actions::SPACE_TYPE_EOA, &[]);

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.to_id, space_id.to_vec());
        assert_eq!(action.action, actions::SPACE_TYPE_DECLARED.to_vec());
        assert_eq!(action.topic, actions::SPACE_TYPE_EOA.to_vec());
    }

    #[test]
    fn test_personal_space_registered_sequence() {
        let space_id = make_id(0x01);
        let owner = make_address(0xAA);
        let events = personal_space_registered(space_id, owner);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, actions::SPACE_REGISTERED.to_vec());
        assert_eq!(events[0].topic, owner.to_vec()); // owner in topic
        assert_eq!(events[1].action, actions::SPACE_TYPE_DECLARED.to_vec());
        assert_eq!(events[1].topic, actions::SPACE_TYPE_EOA.to_vec());
    }

    #[test]
    fn test_dao_space_initialized_sequence() {
        let space_id = make_id(0x01);
        let dao_address = make_address(0xDA);
        let editor1: [u8; 20] = [0xAA; 20];
        let member1: [u8; 20] = [0xBB; 20];
        let events = dao_space_initialized(space_id, dao_address, None, &[editor1], &[member1]);

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].action, actions::SPACE_REGISTERED.to_vec());
        assert_eq!(events[0].topic, dao_address.to_vec()); // DAO address in topic
        assert_eq!(events[1].action, actions::SPACE_TYPE_DECLARED.to_vec());
        assert_eq!(events[1].topic, actions::SPACE_TYPE_DAO.to_vec());
        assert_eq!(events[1].data, b"1.0.0".to_vec()); // version in data
        assert_eq!(events[2].action, actions::EDITOR_ADDED.to_vec());
        assert_eq!(events[3].action, actions::MEMBER_ADDED.to_vec());
    }

    #[test]
    fn test_dao_space_initialized_with_initial_edits() {
        let space_id = make_id(0x01);
        let dao_address = make_address(0xDA);
        let events = dao_space_initialized(space_id, dao_address, Some("QmInitialEdits"), &[], &[]);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].action, actions::SPACE_REGISTERED.to_vec());
        assert_eq!(events[1].action, actions::SPACE_TYPE_DECLARED.to_vec());
        assert_eq!(events[2].action, actions::EDITS_PUBLISHED.to_vec());
        assert_eq!(events[2].data, b"QmInitialEdits".to_vec());
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
        let space_type_count = actions
            .iter()
            .filter(|a| a.action == actions::SPACE_TYPE_DECLARED.to_vec())
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
        let proposal_settings_count = actions
            .iter()
            .filter(|a| a.action == actions::PROPOSAL_SETTINGS_SELECTED.to_vec())
            .count();

        // 18 spaces: 11 canonical + 7 non-canonical (each has SPACE_ID_REGISTERED + SPACE_TYPE_DECLARED)
        assert_eq!(space_count, 18);
        assert_eq!(space_type_count, 18);
        // 10 verified subspaces
        assert_eq!(verified_count, 10);
        // 4 related subspaces
        assert_eq!(related_count, 4);
        // 5 topic declarations
        assert_eq!(topic_declared_count, 5);
        // 6 edits
        assert_eq!(edit_count, 6);
        // 7 proposals with settings (one for each proposal: add_member, remove_member, add_editor, remove_editor, flag, unflag, publish)
        assert_eq!(proposal_settings_count, 7);
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
        assert_eq!(action.to_id, space_id.to_vec()); // from_id == to_id for DAOSpace
        assert_eq!(action.action, actions::PROPOSAL_CREATED.to_vec());
        // Topic is proposal_id padded to 32 bytes
        assert_eq!(action.topic.len(), 32);
        assert_eq!(&action.topic[..16], &proposal_id);
        // Data should not be empty - it contains ABI-encoded (bytes16, uint8, Action[])
        assert!(!action.data.is_empty());
    }

    #[test]
    fn test_proposal_created_slow_path_format() {
        let space_id = make_id(0x01);
        let proposal_id = make_proposal_id(0xA2);
        let member_address = [0xCC; 20];

        let action = proposal_created(
            space_id,
            proposal_id,
            VotingMode::Slow,
            vec![ProposalAction::add_member(member_address)],
        );

        assert!(!action.data.is_empty());
        // Key assertion: data is not empty and can be decoded
    }

    #[test]
    fn test_proposal_settings_selected_format() {
        let space_id = make_id(0x01);
        let proposal_id = make_proposal_id(0xA1);

        let action = proposal_settings_used(space_id, proposal_id, 0, 86400, 50, 60);

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.to_id, space_id.to_vec()); // from_id == to_id for DAOSpace
        assert_eq!(action.action, actions::PROPOSAL_SETTINGS_SELECTED.to_vec());
        // Topic is proposal_id padded to 32 bytes
        assert_eq!(action.topic.len(), 32);
        assert_eq!(&action.topic[..16], &proposal_id);
        // Data should contain ABI-encoded settings
        assert!(!action.data.is_empty());
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
        // Topic is proposal_id padded to 32 bytes
        assert_eq!(action.topic.len(), 32);
        assert_eq!(&action.topic[..16], &proposal_id);
        // Data should contain encoded (bytes16 proposalId, uint8 voteOption)
        // ABI-encoded: 32 bytes for bytes16 (padded) + 32 bytes for uint8 (padded)
        assert_eq!(action.data.len(), 64);
    }

    #[test]
    fn test_proposal_executed_format() {
        let space_id = make_id(0x01);
        let proposal_id = make_proposal_id(0xA1);

        let action = proposal_executed(space_id, proposal_id);

        assert_eq!(action.from_id, space_id.to_vec());
        assert_eq!(action.to_id, space_id.to_vec()); // from_id == to_id for DAOSpace
        assert_eq!(action.action, actions::PROPOSAL_EXECUTED.to_vec());
        // Topic is proposal_id padded to 32 bytes
        assert_eq!(action.topic.len(), 32);
        assert_eq!(&action.topic[..16], &proposal_id);
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

    #[test]
    fn test_proposal_action_remove_member() {
        let member_address = [0xBB; 20];
        let action = ProposalAction::remove_member(member_address);

        // Calldata should start with REMOVE_MEMBER selector
        assert_eq!(&action.data[0..4], &selectors::REMOVE_MEMBER);
        // Followed by padded address
        assert_eq!(&action.data[4..16], &[0u8; 12]);
        assert_eq!(&action.data[16..36], &member_address);
    }

    #[test]
    fn test_proposal_action_add_editor() {
        let editor_address = [0xCC; 20];
        let action = ProposalAction::add_editor(editor_address);

        // Calldata should start with ADD_EDITOR selector
        assert_eq!(&action.data[0..4], &selectors::ADD_EDITOR);
        // Followed by padded address
        assert_eq!(&action.data[4..16], &[0u8; 12]);
        assert_eq!(&action.data[16..36], &editor_address);
    }

    #[test]
    fn test_proposal_action_remove_editor() {
        let editor_address = [0xDD; 20];
        let action = ProposalAction::remove_editor(editor_address);

        // Calldata should start with REMOVE_EDITOR selector
        assert_eq!(&action.data[0..4], &selectors::REMOVE_EDITOR);
        // Followed by padded address
        assert_eq!(&action.data[4..16], &[0u8; 12]);
        assert_eq!(&action.data[16..36], &editor_address);
    }

    #[test]
    fn test_proposal_action_flag() {
        let target = [0xAA; 32];
        let content_uri = b"ipfs://QmTest";
        let action = ProposalAction::flag(target, content_uri);

        // Calldata should start with FLAG selector
        assert_eq!(&action.data[0..4], &selectors::FLAG);
        // Data should be non-empty (ABI-encoded (bytes32, bytes))
        assert!(action.data.len() > 4);
    }

    #[test]
    fn test_proposal_action_unflag() {
        let target = [0xBB; 32];
        let content_uri = b"ipfs://QmTest2";
        let action = ProposalAction::unflag(target, content_uri);

        // Calldata should start with UNFLAG selector
        assert_eq!(&action.data[0..4], &selectors::UNFLAG);
        // Data should be non-empty (ABI-encoded (bytes32, bytes))
        assert!(action.data.len() > 4);
    }
}
