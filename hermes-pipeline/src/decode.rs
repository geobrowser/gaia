//! ABI decoding for action data fields.
//!
//! This module provides functions to decode ABI-encoded data from blockchain actions
//! into their typed representations.

use alloy::sol;
use alloy::sol_types::{SolType, sol_data};
use thiserror::Error;

/// Errors that can occur during ABI decoding.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("Failed to decode ABI data: {0}")]
    AbiDecode(String),

    #[error("Data too short: expected at least {expected} bytes, got {actual}")]
    DataTooShort { expected: usize, actual: usize },

    #[error("Invalid UTF-8 in decoded string: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),
}

// ============================================================================
// DAOSpace Function Selectors
// ============================================================================

/// Function selectors for DAOSpace contract actions.
/// These are the first 4 bytes of keccak256(function_signature).
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
    /// unrestrictSpace(address) - unrestricts a space from fast path
    pub const UNRESTRICT_SPACE: [u8; 4] = [0xb2, 0xc4, 0x36, 0xba];
    /// updateVotingSettings((uint256,uint256,uint256,uint256))
    pub const UPDATE_VOTING_SETTINGS: [u8; 4] = [0xd2, 0x1e, 0x85, 0x41];
    /// ping(bytes32,bytes32,bytes)
    pub const PING: [u8; 4] = [0xc7, 0x0d, 0x82, 0x82];
}

/// Type of action decoded from calldata function selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalActionType {
    Unknown,
    AddMember,
    RemoveMember,
    AddEditor,
    RemoveEditor,
    Publish,
    Flag,
    Unflag,
    UnrestrictSpace,
    UpdateVotingSettings,
    Ping,
}

impl ProposalActionType {
    /// Decode action type from calldata by matching the function selector.
    pub fn from_calldata(calldata: &[u8]) -> Self {
        if calldata.len() < 4 {
            return Self::Unknown;
        }

        let selector: [u8; 4] = calldata[0..4].try_into().unwrap_or([0; 4]);

        match selector {
            selectors::ADD_MEMBER => Self::AddMember,
            selectors::REMOVE_MEMBER => Self::RemoveMember,
            selectors::ADD_EDITOR => Self::AddEditor,
            selectors::REMOVE_EDITOR => Self::RemoveEditor,
            selectors::PUBLISH => Self::Publish,
            selectors::FLAG => Self::Flag,
            selectors::UNFLAG => Self::Unflag,
            selectors::UNRESTRICT_SPACE => Self::UnrestrictSpace,
            selectors::UPDATE_VOTING_SETTINGS => Self::UpdateVotingSettings,
            selectors::PING => Self::Ping,
            _ => Self::Unknown,
        }
    }
}

/// Decode an address argument from calldata.
///
/// For functions like addMember(address), the calldata is:
/// - 4 bytes: function selector
/// - 32 bytes: ABI-encoded address (left-padded with zeros)
///
/// Returns the 20-byte address.
pub fn decode_address_arg(calldata: &[u8]) -> Option<Vec<u8>> {
    // Need at least selector (4) + padded address (32) = 36 bytes
    if calldata.len() < 36 {
        return None;
    }

    // ABI-encoded address is 32 bytes, with address in last 20 bytes
    // bytes 4..16 are padding (zeros), bytes 16..36 are the address
    Some(calldata[16..36].to_vec())
}

/// Decoded publish action arguments.
#[derive(Debug, Clone)]
pub struct PublishArgs {
    /// Content URI (IPFS hash, etc.)
    pub content_uri: String,
    /// Edit metadata
    pub metadata: Vec<u8>,
}

/// Decode publish(bytes32, bytes, bytes) calldata.
///
/// The calldata is:
/// - 4 bytes: function selector
/// - ABI-encoded (bytes32 topic, bytes contentUri, bytes metadata)
///
/// We skip the topic and return contentUri and metadata.
pub fn decode_publish_args(calldata: &[u8]) -> Result<PublishArgs, DecodeError> {
    if calldata.len() < 4 {
        return Err(DecodeError::DataTooShort {
            expected: 4,
            actual: calldata.len(),
        });
    }

    // Skip the 4-byte selector
    let data = &calldata[4..];

    // Decode (bytes32, bytes, bytes)
    type PublishArgsType = sol! { (bytes32, bytes, bytes) };
    let (_topic, content_uri, metadata) =
        PublishArgsType::abi_decode(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    let content_uri_str = String::from_utf8(content_uri.to_vec()).map_err(DecodeError::from)?;

    Ok(PublishArgs {
        content_uri: content_uri_str,
        metadata: metadata.to_vec(),
    })
}

/// Decoded flag/unflag action arguments.
#[derive(Debug, Clone)]
pub struct FlagArgs {
    /// Content identifier being flagged/unflagged
    pub content_id: Vec<u8>,
}

/// Decode flag(bytes32, bytes) or unflag(bytes32, bytes) calldata.
///
/// The calldata is:
/// - 4 bytes: function selector
/// - ABI-encoded (bytes32 topic, bytes contentId)
///
/// We skip the topic and return contentId.
pub fn decode_flag_args(calldata: &[u8]) -> Result<FlagArgs, DecodeError> {
    if calldata.len() < 4 {
        return Err(DecodeError::DataTooShort {
            expected: 4,
            actual: calldata.len(),
        });
    }

    // Skip the 4-byte selector
    let data = &calldata[4..];

    // Decode (bytes32, bytes)
    type FlagArgsType = sol! { (bytes32, bytes) };
    let (_topic, content_id) =
        FlagArgsType::abi_decode(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(FlagArgs {
        content_id: content_id.to_vec(),
    })
}

/// Decoded voting settings update arguments.
#[derive(Debug, Clone)]
pub struct VotingSettingsArgs {
    /// Minimum total votes required
    pub quorum: u64,
    /// Fast path: absolute YES votes needed
    pub fast_threshold: u64,
    /// Slow path: percentage of RATIO_BASE (10,000,000)
    pub slow_threshold: u64,
    /// Voting duration
    pub duration: u64,
}

/// Decode updateVotingSettings((uint256,uint256,uint256,uint256)) calldata.
///
/// The calldata is:
/// - 4 bytes: function selector
/// - ABI-encoded tuple (quorum, fastThreshold, slowThreshold, duration)
pub fn decode_voting_settings_args(calldata: &[u8]) -> Result<VotingSettingsArgs, DecodeError> {
    if calldata.len() < 4 {
        return Err(DecodeError::DataTooShort {
            expected: 4,
            actual: calldata.len(),
        });
    }

    // Skip the 4-byte selector
    let data = &calldata[4..];

    // Decode ((uint256, uint256, uint256, uint256))
    // Note: Solidity struct is encoded as a tuple
    type VotingSettingsArgsType = sol! { (uint256, uint256, uint256, uint256) };
    let (quorum, fast_threshold, slow_threshold, duration) =
        VotingSettingsArgsType::abi_decode(data)
            .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    // Convert U256 to u64, saturating if too large
    Ok(VotingSettingsArgs {
        quorum: quorum.try_into().unwrap_or(u64::MAX),
        fast_threshold: fast_threshold.try_into().unwrap_or(u64::MAX),
        slow_threshold: slow_threshold.try_into().unwrap_or(u64::MAX),
        duration: duration.try_into().unwrap_or(u64::MAX),
    })
}

// ============================================================================
// Solidity Type Definitions
// ============================================================================

sol! {
    /// Action to be executed when a proposal passes.
    #[derive(Debug)]
    struct Action {
        address to;
        uint256 value;
        bytes data;
    }
}

// Type aliases for ABI decoding
// PROPOSAL_CREATED: abi.encode(bytes16 proposalId, VotingMode, Action[])
type ProposalCreatedDataType = sol! { (bytes16, uint8, Action[]) };
// PROPOSAL_SETTINGS_USED: abi.encode(startDate, lastDate, votingMode, quorum, supportThreshold)
type ProposalSettingsUsedDataType = sol! { (uint64, uint64, uint8, uint256, uint256) };
// PROPOSAL_VOTED: abi.encode(bytes16 proposalId, VoteOption)
type ProposalVotedDataType = sol! { (bytes16, uint8) };
type VoteDataType = sol! { (uint16, bytes16, bytes16) };
#[allow(dead_code)] // Prepared for future EDITS_PUBLISHED decoding
type EditsPublishedDataType = sol! { (bytes, bytes) };

// ============================================================================
// Governance Decoding
// ============================================================================

/// Decoded proposal creation data.
#[derive(Debug, Clone)]
pub struct ProposalCreatedData {
    /// Proposal ID (16 bytes).
    pub proposal_id: Vec<u8>,
    /// Voting mode (0=Slow, 1=Fast).
    pub voting_mode: u8,
    /// Actions to execute if proposal passes.
    pub actions: Vec<ProposalAction>,
}

/// A single action in a proposal.
#[derive(Debug, Clone)]
pub struct ProposalAction {
    /// Target contract address (20 bytes).
    pub to: Vec<u8>,
    /// ETH value to send (32 bytes, big-endian).
    pub value: Vec<u8>,
    /// Calldata (function selector + encoded args).
    pub data: Vec<u8>,
}

/// Decoded proposal settings from PROPOSAL_SETTINGS_USED.
#[derive(Debug, Clone)]
pub struct ProposalSettingsData {
    /// Block timestamp when voting starts.
    pub start_date: u64,
    /// Block timestamp when voting ends.
    pub last_date: u64,
    /// Voting mode (0=Fast, 1=Slow).
    pub voting_mode: u8,
    /// Minimum total votes for slow path (quorum).
    pub quorum: u64,
    /// Support threshold (flat for fast path, percentage for slow path).
    pub support_threshold: u64,
}

/// Decode PROPOSAL_CREATED data.
///
/// Encoding: `abi.encode(bytes16 proposalId, VotingMode, Action[])`
pub fn decode_proposal_created(data: &[u8]) -> Result<ProposalCreatedData, DecodeError> {
    if data.is_empty() {
        return Ok(ProposalCreatedData {
            proposal_id: vec![0; 16],
            voting_mode: 0,
            actions: vec![],
        });
    }

    let (proposal_id, voting_mode, actions) = ProposalCreatedDataType::abi_decode(data)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    let actions = actions
        .into_iter()
        .map(|action| ProposalAction {
            to: action.to.as_slice().to_vec(),
            value: action.value.to_be_bytes_vec(),
            data: action.data.to_vec(),
        })
        .collect();

    Ok(ProposalCreatedData {
        proposal_id: proposal_id.to_vec(),
        voting_mode,
        actions,
    })
}

/// Decode PROPOSAL_SETTINGS_USED data.
///
/// Encoding: `abi.encode(startDate, lastDate, votingMode, quorum, supportThreshold)`
pub fn decode_proposal_settings_used(data: &[u8]) -> Result<ProposalSettingsData, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::DataTooShort {
            expected: 160, // 5 * 32 bytes for uint64, uint64, uint8, uint256, uint256
            actual: 0,
        });
    }

    let (start_date, last_date, voting_mode, quorum, support_threshold) =
        ProposalSettingsUsedDataType::abi_decode(data)
            .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    // Convert U256 to u64, saturating if too large
    let quorum_u64 = quorum.try_into().unwrap_or(u64::MAX);
    let threshold_u64 = support_threshold.try_into().unwrap_or(u64::MAX);

    Ok(ProposalSettingsData {
        start_date,
        last_date,
        voting_mode,
        quorum: quorum_u64,
        support_threshold: threshold_u64,
    })
}

/// Decoded proposal vote data.
#[derive(Debug, Clone)]
pub struct ProposalVotedData {
    /// Proposal ID (16 bytes).
    #[allow(dead_code)] // Available for callers who need it
    pub proposal_id: Vec<u8>,
    /// Vote option (0=None, 1=Abstain, 2=Yes, 3=No).
    pub vote: u8,
}

/// Decode PROPOSAL_VOTED data.
///
/// Encoding: `abi.encode(bytes16 proposalId, VoteOption)`
pub fn decode_proposal_voted(data: &[u8]) -> Result<ProposalVotedData, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::DataTooShort {
            expected: 48, // bytes16 + uint8 (padded)
            actual: 0,
        });
    }

    let (proposal_id, vote_option) = ProposalVotedDataType::abi_decode(data)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(ProposalVotedData {
        proposal_id: proposal_id.to_vec(),
        vote: vote_option,
    })
}

// ============================================================================
// Permissionless Voting Decoding
// ============================================================================

/// Decoded vote data for permissionless voting.
#[derive(Debug, Clone)]
pub struct VoteData {
    /// Vote format version.
    pub version: u16,
    /// Group identifier (16 bytes).
    pub group_id: Vec<u8>,
    /// Space point of view (16 bytes).
    pub space_pov: Vec<u8>,
}

/// Decode UPVOTED/DOWNVOTED/UNVOTED data.
///
/// Encoding: `abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))`
pub fn decode_vote_data(data: &[u8]) -> Result<VoteData, DecodeError> {
    if data.is_empty() {
        return Ok(VoteData {
            version: 0,
            group_id: vec![0; 16],
            space_pov: vec![0; 16],
        });
    }

    let (version, group_id, space_pov) =
        VoteDataType::abi_decode(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(VoteData {
        version,
        group_id: group_id.to_vec(),
        space_pov: space_pov.to_vec(),
    })
}

// ============================================================================
// Membership Decoding
// ============================================================================

/// Decode ABI-encoded address from data field.
///
/// ZC16 format for EDITOR_ADDED, EDITOR_REMOVED, MEMBER_ADDED, MEMBER_REMOVED:
/// - topic: bytes32(spaceId) - target space ID (unused for address extraction)
/// - data: abi.encode(address) - 32 bytes, address in last 20 bytes
///
/// Returns the 20-byte address.
pub fn decode_address(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    // ABI-encoded address is 32 bytes: 12 bytes padding + 20 bytes address
    if data.len() < 32 {
        return Err(DecodeError::DataTooShort {
            expected: 32,
            actual: data.len(),
        });
    }

    // Extract the 20-byte address from the last 20 bytes
    Ok(data[12..32].to_vec())
}

// ============================================================================
// Content Decoding
// ============================================================================

/// Decode TOPIC_DECLARED data.
///
/// Encoding: `abi.encode(bytes16(topicId))`
pub fn decode_topic_declared(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    if data.is_empty() {
        return Ok(vec![0; 16]);
    }

    let topic_id = sol_data::FixedBytes::<16>::abi_decode(data)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(topic_id.to_vec())
}

/// Decoded edits published data.
#[allow(dead_code)] // Prepared for future EDITS_PUBLISHED decoding
#[derive(Debug, Clone)]
pub struct EditsPublishedData {
    /// Content URI (e.g., IPFS hash).
    pub content_uri: Vec<u8>,
    /// Edit metadata.
    pub metadata: Vec<u8>,
}

/// Decode EDITS_PUBLISHED data.
///
/// Encoding: `abi.encode(bytes(editsContentUri), bytes(editsMetadata))`
#[allow(dead_code)] // Prepared for future EDITS_PUBLISHED decoding
pub fn decode_edits_published(data: &[u8]) -> Result<EditsPublishedData, DecodeError> {
    if data.is_empty() {
        return Ok(EditsPublishedData {
            content_uri: vec![],
            metadata: vec![],
        });
    }

    let (content_uri, metadata) = EditsPublishedDataType::abi_decode(data)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(EditsPublishedData {
        content_uri: content_uri.to_vec(),
        metadata: metadata.to_vec(),
    })
}

// ============================================================================
// Moderation Decoding
// ============================================================================

/// Decode FLAGGED/UNFLAGGED data.
///
/// Encoding: `abi.encode(bytes(uri))`
pub fn decode_flag_data(data: &[u8]) -> Result<String, DecodeError> {
    if data.is_empty() {
        return Ok(String::new());
    }

    // Single bytes element - decode as sol_data::Bytes type
    let uri_bytes =
        sol_data::Bytes::abi_decode(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    String::from_utf8(uri_bytes.to_vec()).map_err(DecodeError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, Bytes as PrimBytes, FixedBytes, U256};

    #[test]
    fn test_decode_proposal_created_empty() {
        let result = decode_proposal_created(&[]).unwrap();
        assert!(result.actions.is_empty());
        assert_eq!(result.voting_mode, 0);
    }

    #[test]
    fn test_decode_proposal_created() {
        // Create test data using the Action struct
        let proposal_id = FixedBytes::<16>::from([0xAB; 16]);
        let actions = vec![Action {
            to: Address::ZERO,
            value: U256::from(1000u64),
            data: vec![1, 2, 3].into(),
        }];
        let voting_mode = 1u8; // Slow
        let encoded = ProposalCreatedDataType::abi_encode(&(proposal_id, voting_mode, actions));

        let result = decode_proposal_created(&encoded).unwrap();
        assert_eq!(result.proposal_id, vec![0xAB; 16]);
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].to, Address::ZERO.as_slice().to_vec());
        assert_eq!(result.voting_mode, 1);
    }

    #[test]
    fn test_decode_proposal_voted() {
        let proposal_id = FixedBytes::<16>::from([0xCD; 16]);
        let vote_option = 2u8; // No
        let encoded = ProposalVotedDataType::abi_encode(&(proposal_id, vote_option));

        let result = decode_proposal_voted(&encoded).unwrap();
        assert_eq!(result.proposal_id, vec![0xCD; 16]);
        assert_eq!(result.vote, 2);
    }

    #[test]
    fn test_decode_vote_data() {
        let version = 1u16;
        let group_id = FixedBytes::<16>::from([2u8; 16]);
        let space_pov = FixedBytes::<16>::from([3u8; 16]);
        let encoded = VoteDataType::abi_encode(&(version, group_id, space_pov));

        let result = decode_vote_data(&encoded).unwrap();
        assert_eq!(result.version, 1);
        assert_eq!(result.group_id, vec![2u8; 16]);
        assert_eq!(result.space_pov, vec![3u8; 16]);
    }

    #[test]
    fn test_decode_vote_data_empty() {
        let result = decode_vote_data(&[]).unwrap();
        assert_eq!(result.version, 0);
        assert_eq!(result.group_id.len(), 16);
        assert_eq!(result.space_pov.len(), 16);
    }

    #[test]
    fn test_decode_topic_declared() {
        let topic_id = FixedBytes::<16>::from([1u8; 16]);
        let encoded = sol_data::FixedBytes::<16>::abi_encode(&topic_id);

        let result = decode_topic_declared(&encoded).unwrap();
        assert_eq!(result, vec![1u8; 16]);
    }

    #[test]
    fn test_decode_topic_declared_empty() {
        let result = decode_topic_declared(&[]).unwrap();
        assert_eq!(result, vec![0u8; 16]);
    }

    #[test]
    fn test_decode_flag_data() {
        let uri: PrimBytes = "ipfs://QmTest123".as_bytes().to_vec().into();
        let encoded = sol_data::Bytes::abi_encode(&uri);

        let result = decode_flag_data(&encoded).unwrap();
        assert_eq!(result, "ipfs://QmTest123");
    }

    #[test]
    fn test_decode_flag_data_empty() {
        let result = decode_flag_data(&[]).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_proposal_action_type_from_calldata() {
        // Test known selectors
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::ADD_MEMBER),
            ProposalActionType::AddMember
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::REMOVE_MEMBER),
            ProposalActionType::RemoveMember
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::ADD_EDITOR),
            ProposalActionType::AddEditor
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::REMOVE_EDITOR),
            ProposalActionType::RemoveEditor
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::PUBLISH),
            ProposalActionType::Publish
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::FLAG),
            ProposalActionType::Flag
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::UNFLAG),
            ProposalActionType::Unflag
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::UNRESTRICT_SPACE),
            ProposalActionType::UnrestrictSpace
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::UPDATE_VOTING_SETTINGS),
            ProposalActionType::UpdateVotingSettings
        );
        assert_eq!(
            ProposalActionType::from_calldata(&selectors::PING),
            ProposalActionType::Ping
        );

        // Test with extra data after selector
        let mut calldata_with_args = selectors::ADD_MEMBER.to_vec();
        calldata_with_args.extend_from_slice(&[0u8; 32]); // padded address arg
        assert_eq!(
            ProposalActionType::from_calldata(&calldata_with_args),
            ProposalActionType::AddMember
        );

        // Test unknown selector
        assert_eq!(
            ProposalActionType::from_calldata(&[0xde, 0xad, 0xbe, 0xef]),
            ProposalActionType::Unknown
        );

        // Test too short calldata
        assert_eq!(
            ProposalActionType::from_calldata(&[0xca, 0x6d]),
            ProposalActionType::Unknown
        );
        assert_eq!(
            ProposalActionType::from_calldata(&[]),
            ProposalActionType::Unknown
        );
    }

    #[test]
    fn test_decode_address_arg() {
        // Create calldata for addMember(0x1111...1111)
        let mut calldata = selectors::ADD_MEMBER.to_vec();
        // ABI-encoded address: 12 bytes padding + 20 bytes address
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(&[0x11u8; 20]);

        let result = decode_address_arg(&calldata);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), vec![0x11u8; 20]);
    }

    #[test]
    fn test_decode_address_arg_too_short() {
        // Only selector, no address
        let calldata = selectors::ADD_MEMBER.to_vec();
        assert!(decode_address_arg(&calldata).is_none());

        // Partially filled
        let mut calldata = selectors::ADD_MEMBER.to_vec();
        calldata.extend_from_slice(&[0u8; 20]); // Only 24 bytes total
        assert!(decode_address_arg(&calldata).is_none());
    }

    #[test]
    fn test_decode_address() {
        // ABI-encoded address: 12 bytes padding + 20 bytes address
        let mut data = vec![0u8; 12];
        data.extend_from_slice(&[0xAA; 20]);

        let result = decode_address(&data).unwrap();
        assert_eq!(result, vec![0xAA; 20]);
    }

    #[test]
    fn test_decode_address_too_short() {
        let data = vec![0u8; 20]; // Too short
        let result = decode_address(&data);
        assert!(matches!(result, Err(DecodeError::DataTooShort { .. })));
    }
}
