//! ABI decoding for action data fields.
//!
//! This module provides functions to decode ABI-encoded data from blockchain actions
//! into their typed representations.

use alloy::sol;
use alloy::sol_types::{SolType, sol_data};
use ethabi::{ParamType, Token};
use std::borrow::Cow;
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
/// Note: DAOSpace uses bytes16 for space IDs, not address.
pub mod selectors {
    /// addMember(bytes16)
    pub const ADD_MEMBER: [u8; 4] = [0x2a, 0xfb, 0xe3, 0x50];
    /// removeMember(bytes16)
    pub const REMOVE_MEMBER: [u8; 4] = [0x35, 0xfa, 0x4f, 0x95];
    /// addEditor(bytes16)
    pub const ADD_EDITOR: [u8; 4] = [0x1c, 0xc8, 0xe1, 0x8a];
    /// removeEditor(bytes16)
    pub const REMOVE_EDITOR: [u8; 4] = [0x72, 0x3a, 0xe1, 0xe8];
    /// publish(bytes32,bytes,bytes)
    pub const PUBLISH: [u8; 4] = [0x6b, 0x47, 0xf6, 0x1a];
    /// flag(bytes32,bytes)
    pub const FLAG: [u8; 4] = [0xfe, 0x1e, 0x30, 0x42];
    /// unflag(bytes32,bytes)
    pub const UNFLAG: [u8; 4] = [0xc6, 0x96, 0x84, 0x0f];
    /// unrestrictSpace(bytes16)
    pub const UNRESTRICT_SPACE: [u8; 4] = [0xd6, 0xf8, 0x43, 0x2f];
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

/// Decode a bytes16 (space ID) argument from calldata.
///
/// For functions like addMember(bytes16), the calldata is:
/// - 4 bytes: function selector
/// - 32 bytes: ABI-encoded bytes16 (right-padded with zeros)
///
/// Returns the 16-byte space ID.
pub fn decode_space_id_arg(calldata: &[u8]) -> Option<Vec<u8>> {
    // Need at least selector (4) + padded bytes16 (32) = 36 bytes
    if calldata.len() < 36 {
        return None;
    }

    // ABI-encoded bytes16 is 32 bytes, with bytes16 in first 16 bytes
    // bytes 4..20 are the bytes16, bytes 20..36 are padding (zeros)
    Some(calldata[4..20].to_vec())
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

    let content_uri_str = decode_utf8_bytes(content_uri.to_vec())?;

    Ok(PublishArgs {
        content_uri: content_uri_str,
        metadata: metadata.to_vec(),
    })
}

fn decode_utf8_bytes(mut data: Vec<u8>) -> Result<String, DecodeError> {
    for _ in 0..2 {
        if let Some(unwrapped) = unwrap_bytes_once(&data) {
            data = unwrapped;
        } else {
            break;
        }
    }

    data.retain(|b| *b != 0);
    String::from_utf8(data).map_err(DecodeError::from)
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
#[allow(dead_code)] // Used in tests for encoding
type ProposalCreatedDataType = sol! { (bytes16, uint8, Action[]) };
// PROPOSAL_SETTINGS_USED: abi.encode(startDate, lastDate, votingMode, quorum, supportThreshold)
// Note: onchain start/last dates are uint256 timestamps.
#[allow(dead_code)] // Used in tests for encoding
type ProposalSettingsUsedDataType = sol! { (uint256, uint256, uint8, uint256, uint256) };
// PROPOSAL_VOTED: abi.encode(bytes16 proposalId, VoteOption)
type ProposalVotedDataType = sol! { (bytes16, uint8) };
type VoteDataType = sol! { (uint16, bytes16, bytes16) };
#[allow(dead_code)] // Prepared for future EDITS_PUBLISHED decoding
type EditsPublishedDataType = sol! { (bytes, bytes) };
#[allow(dead_code)] // Reserved for future use
type WrappedBytesType = sol! { bytes };

fn maybe_unwrap_bytes(data: &[u8]) -> Cow<'_, [u8]> {
    if data.len() < 64 {
        return Cow::Borrowed(data);
    }

    if data[0..24].iter().any(|b| *b != 0) || data[32..56].iter().any(|b| *b != 0) {
        return Cow::Borrowed(data);
    }

    let offset = u64::from_be_bytes(data[24..32].try_into().unwrap());
    if offset != 32 {
        return Cow::Borrowed(data);
    }

    let len = u64::from_be_bytes(data[56..64].try_into().unwrap()) as usize;
    let start = 64;
    let end = start + len;
    if end > data.len() {
        return Cow::Borrowed(data);
    }

    Cow::Borrowed(&data[start..end])
}

fn unwrap_bytes_once(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 64 {
        return None;
    }

    if data[0..24].iter().any(|b| *b != 0) || data[32..56].iter().any(|b| *b != 0) {
        return None;
    }

    let offset = u64::from_be_bytes(data[24..32].try_into().ok()?) as usize;
    if offset != 32 {
        return None;
    }

    let len = u64::from_be_bytes(data[56..64].try_into().ok()?) as usize;
    let start = 64;
    let end = start + len;
    if end > data.len() {
        return None;
    }

    Some(data[start..end].to_vec())
}

pub fn unwrap_debug_chain(data: &[u8], max_levels: usize) -> Vec<Vec<u8>> {
    let mut chain: Vec<Vec<u8>> = Vec::new();
    chain.push(data.to_vec());

    for _ in 0..max_levels {
        let Some(current) = chain.last() else {
            break;
        };
        let current = current.as_slice();
        let Some(next) = unwrap_bytes_once(current) else {
            break;
        };
        chain.push(next);
    }

    chain
}

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
pub fn decode_proposal_created(data: &[u8]) -> Result<(ProposalCreatedData, u8), DecodeError> {
    if data.is_empty() {
        return Ok((
            ProposalCreatedData {
                proposal_id: vec![0; 16],
                voting_mode: 0,
                actions: vec![],
            },
            0,
        ));
    }

    let mut current = maybe_unwrap_bytes(data);
    let mut unwrap_level = if current.len() != data.len() || current.as_ptr() != data.as_ptr() {
        1
    } else {
        0
    };

    for _ in 0..=1 {
        if let Ok(decoded) = decode_proposal_created_inner(&current) {
            return Ok((decoded, unwrap_level));
        }

        let Some(unwrapped) = unwrap_bytes_once(current.as_ref()) else {
            break;
        };
        current = Cow::Owned(unwrapped);
        unwrap_level = unwrap_level.saturating_add(1);
    }

    Err(DecodeError::AbiDecode(
        "Failed to decode proposal created data".to_string(),
    ))
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

    let current = maybe_unwrap_bytes(data);

    let decoded = match decode_proposal_settings_used_inner(&current) {
        Ok(decoded) => decoded,
        Err(_) => {
            let Some(unwrapped) = unwrap_bytes_once(current.as_ref()) else {
                return Err(DecodeError::AbiDecode(
                    "Failed to decode proposal settings used".to_string(),
                ));
            };
            let current = Cow::Owned(unwrapped);
            decode_proposal_settings_used_inner(&current).map_err(|_| {
                DecodeError::AbiDecode("Failed to decode proposal settings used".to_string())
            })?
        }
    };

    let (start_date, last_date, voting_mode, quorum, support_threshold) = decoded;

    Ok(ProposalSettingsData {
        start_date,
        last_date,
        voting_mode,
        quorum,
        support_threshold,
    })
}

fn decode_proposal_created_inner(data: &[u8]) -> Result<ProposalCreatedData, DecodeError> {
    let params = [
        ParamType::FixedBytes(16),
        ParamType::Uint(8),
        ParamType::Array(Box::new(ParamType::Tuple(vec![
            ParamType::Address,
            ParamType::Uint(256),
            ParamType::Bytes,
        ]))),
    ];

    let tokens =
        ethabi::decode(&params, data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    let proposal_id = match &tokens[0] {
        Token::FixedBytes(bytes) if bytes.len() == 16 => bytes.clone(),
        _ => return Err(DecodeError::AbiDecode("Invalid proposal_id".to_string())),
    };

    let voting_mode = match &tokens[1] {
        Token::Uint(value) => {
            let v = value.low_u32();
            if v > u8::MAX as u32 {
                return Err(DecodeError::AbiDecode("Invalid voting_mode".to_string()));
            }
            v as u8
        }
        _ => return Err(DecodeError::AbiDecode("Invalid voting_mode".to_string())),
    };

    let actions = match &tokens[2] {
        Token::Array(items) => items
            .iter()
            .map(|item| match item {
                Token::Tuple(fields) if fields.len() == 3 => {
                    let to = match &fields[0] {
                        Token::Address(addr) => addr.as_bytes().to_vec(),
                        _ => return Err(DecodeError::AbiDecode("Invalid action.to".to_string())),
                    };
                    let value = match &fields[1] {
                        Token::Uint(v) => {
                            let mut buf = [0u8; 32];
                            v.to_big_endian(&mut buf);
                            buf.to_vec()
                        }
                        _ => {
                            return Err(DecodeError::AbiDecode("Invalid action.value".to_string()));
                        }
                    };
                    let data = match &fields[2] {
                        Token::Bytes(bytes) => bytes.clone(),
                        _ => return Err(DecodeError::AbiDecode("Invalid action.data".to_string())),
                    };
                    Ok(ProposalAction { to, value, data })
                }
                _ => Err(DecodeError::AbiDecode("Invalid action tuple".to_string())),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(DecodeError::AbiDecode("Invalid actions array".to_string())),
    };

    Ok(ProposalCreatedData {
        proposal_id,
        voting_mode,
        actions,
    })
}

fn decode_proposal_settings_used_inner(
    data: &[u8],
) -> Result<(u64, u64, u8, u64, u64), DecodeError> {
    let params = [
        ParamType::Uint(256),
        ParamType::Uint(256),
        ParamType::Uint(8),
        ParamType::Uint(256),
        ParamType::Uint(256),
    ];

    let tokens =
        ethabi::decode(&params, data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    let start = match &tokens[0] {
        Token::Uint(v) => v.as_u64(),
        _ => return Err(DecodeError::AbiDecode("Invalid start_date".to_string())),
    };
    let last = match &tokens[1] {
        Token::Uint(v) => v.as_u64(),
        _ => return Err(DecodeError::AbiDecode("Invalid last_date".to_string())),
    };
    let voting_mode = match &tokens[2] {
        Token::Uint(v) => v.low_u32() as u8,
        _ => return Err(DecodeError::AbiDecode("Invalid voting_mode".to_string())),
    };
    let quorum = match &tokens[3] {
        Token::Uint(v) => v.as_u64(),
        _ => return Err(DecodeError::AbiDecode("Invalid quorum".to_string())),
    };
    let support = match &tokens[4] {
        Token::Uint(v) => v.as_u64(),
        _ => {
            return Err(DecodeError::AbiDecode(
                "Invalid support_threshold".to_string(),
            ));
        }
    };

    Ok((start, last, voting_mode, quorum, support))
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
    use alloy::primitives::{Bytes as PrimBytes, FixedBytes};

    #[test]
    fn test_decode_proposal_created_empty() {
        let (result, unwrap_level) = decode_proposal_created(&[]).unwrap();
        assert_eq!(unwrap_level, 0);
        assert!(result.actions.is_empty());
        assert_eq!(result.voting_mode, 0);
    }

    #[test]
    fn test_decode_proposal_created() {
        // Encode using ethabi to match the decoder
        use ethabi::ethereum_types::U256 as EthU256;

        let proposal_id = vec![0xAB_u8; 16];
        let voting_mode = EthU256::from(1u8);
        let action_tuple = Token::Tuple(vec![
            Token::Address(ethabi::Address::zero()),
            Token::Uint(EthU256::from(1000u64)),
            Token::Bytes(vec![1, 2, 3]),
        ]);

        let encoded = ethabi::encode(&[
            Token::FixedBytes(proposal_id.clone()),
            Token::Uint(voting_mode),
            Token::Array(vec![action_tuple]),
        ]);

        let (result, unwrap_level) = decode_proposal_created(&encoded).unwrap();
        assert_eq!(unwrap_level, 0);
        assert_eq!(result.proposal_id, proposal_id);
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].to, vec![0u8; 20]);
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
    fn test_decode_publish_args_unwraps_content_uri() {
        let uri = b"ipfs://QmUgncZn6KFgv7tnpYcknMkPceSMNFhSYRY95GxX45MYyc".to_vec();
        let wrapped_uri = {
            type WrappedBytes = sol! { bytes };
            WrappedBytes::abi_encode(&PrimBytes::from(uri.clone()))
        };

        type PublishArgsType = sol! { (bytes32, bytes, bytes) };
        let args = PublishArgsType::abi_encode(&(
            [0u8; 32],
            PrimBytes::from(wrapped_uri),
            PrimBytes::default(),
        ));

        let mut calldata = Vec::with_capacity(4 + args.len());
        calldata.extend_from_slice(&selectors::PUBLISH);
        calldata.extend_from_slice(&args);

        let decoded = decode_publish_args(&calldata).unwrap();
        assert_eq!(decoded.content_uri, String::from_utf8(uri).unwrap());
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
    fn test_decode_space_id_arg() {
        // Create calldata for addMember(bytes16)
        let mut calldata = selectors::ADD_MEMBER.to_vec();
        // ABI-encoded bytes16: 16 bytes value + 16 bytes padding
        calldata.extend_from_slice(&[0x11u8; 16]);
        calldata.extend_from_slice(&[0u8; 16]);

        let result = decode_space_id_arg(&calldata);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), vec![0x11u8; 16]);
    }

    #[test]
    fn test_decode_space_id_arg_too_short() {
        // Only selector, no space id
        let calldata = selectors::ADD_MEMBER.to_vec();
        assert!(decode_space_id_arg(&calldata).is_none());

        // Partially filled
        let mut calldata = selectors::ADD_MEMBER.to_vec();
        calldata.extend_from_slice(&[0u8; 20]); // Only 24 bytes total
        assert!(decode_space_id_arg(&calldata).is_none());
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
