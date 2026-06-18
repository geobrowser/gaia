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

    #[error("Data too long: expected at most {expected} bytes, got {actual}")]
    DataTooLong { expected: usize, actual: usize },
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
    /// updateVotingSettings((uint256,uint256,uint256,uint256,uint256,bool,uint256))
    pub const UPDATE_VOTING_SETTINGS: [u8; 4] = [0xf2, 0x2e, 0xc6, 0xb2];
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

/// Decoded voting settings update arguments (V2, 7 fields).
/// Field order matches the Solidity `VotingSettings` struct.
#[derive(Debug, Clone)]
pub struct VotingSettingsArgs {
    /// Slow path late execution threshold (0..RATIO_BASE, % of yes/no votes)
    pub partial_percentage_support_threshold: u64,
    /// Slow path early execution threshold (0..RATIO_BASE, % of total editors)
    pub universal_percentage_support_threshold: u64,
    /// Fast path absolute YES votes needed
    pub flat_support_threshold: u64,
    /// Minimum participating votes for slow path
    pub quorum: u64,
    /// Voting window duration in seconds
    pub duration: u64,
    /// Whether newly added members are restricted from the fast path
    pub disable_fast_path_access_for_new_members: bool,
    /// Seconds after `lastDate` during which a passed proposal may still be executed
    pub execution_grace_period: u64,
}

/// ABI shape for the V2 `VotingSettings` tuple.
/// Used both by `decode_voting_settings_args` (function calldata) and
/// `decode_voting_settings_data` (raw event payload).
type VotingSettingsTuple = sol! { (uint256, uint256, uint256, uint256, uint256, bool, uint256) };

/// Decode `updateVotingSettings((uint256,uint256,uint256,uint256,uint256,bool,uint256))` calldata.
///
/// The calldata is:
/// - 4 bytes: function selector
/// - ABI-encoded 7-field `VotingSettings` tuple
pub fn decode_voting_settings_args(calldata: &[u8]) -> Result<VotingSettingsArgs, DecodeError> {
    if calldata.len() < 4 {
        return Err(DecodeError::DataTooShort {
            expected: 4,
            actual: calldata.len(),
        });
    }

    // Skip the 4-byte selector
    decode_voting_settings_data(&calldata[4..])
}

/// Size in bytes of the raw ABI-encoded `VotingSettings` tuple: 7 static
/// words × 32 bytes each.
const VOTING_SETTINGS_TUPLE_SIZE: usize = 7 * 32;

/// Decode a raw ABI-encoded `VotingSettings` tuple (no function selector).
///
/// Used for the `VOTING_SETTINGS_UPDATED` action event, whose `data` field is
/// `abi.encode(_votingSettings)` without any selector prefix.
///
/// The expected payload is a fixed-size static 7-word tuple (7 × 32 = 224 bytes),
/// so we do not need to speculatively unwrap before trying to decode. The eager
/// `maybe_unwrap_bytes` heuristic can mis-detect a valid raw tuple as an ABI
/// `bytes` envelope (e.g., when `partial_percentage_support_threshold == 32`
/// and `universal_percentage_support_threshold <= 160`), slice the buffer, and
/// drop the event. Invert the order:
pub fn decode_voting_settings_data(data: &[u8]) -> Result<VotingSettingsArgs, DecodeError> {
    if data.len() == VOTING_SETTINGS_TUPLE_SIZE
        && let Ok(args) = decode_voting_settings_data_inner(data)
    {
        return Ok(args);
    }

    let unwrapped_once = maybe_unwrap_bytes(data);
    if let Ok(args) = decode_voting_settings_data_inner(&unwrapped_once) {
        return Ok(args);
    }

    if let Some(unwrapped_twice) = unwrap_bytes_once(unwrapped_once.as_ref())
        && let Ok(args) = decode_voting_settings_data_inner(&unwrapped_twice)
    {
        return Ok(args);
    }

    Err(DecodeError::AbiDecode(
        "Failed to decode voting settings data".to_string(),
    ))
}

fn decode_voting_settings_data_inner(data: &[u8]) -> Result<VotingSettingsArgs, DecodeError> {
    let (
        partial,
        universal,
        flat,
        quorum,
        duration,
        disable_fast_path_access_for_new_members,
        execution_grace_period,
    ) = VotingSettingsTuple::abi_decode(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    // Convert U256 to u64, saturating if too large
    Ok(VotingSettingsArgs {
        partial_percentage_support_threshold: partial.try_into().unwrap_or(u64::MAX),
        universal_percentage_support_threshold: universal.try_into().unwrap_or(u64::MAX),
        flat_support_threshold: flat.try_into().unwrap_or(u64::MAX),
        quorum: quorum.try_into().unwrap_or(u64::MAX),
        duration: duration.try_into().unwrap_or(u64::MAX),
        disable_fast_path_access_for_new_members,
        execution_grace_period: execution_grace_period.try_into().unwrap_or(u64::MAX),
    })
}

/// Decoded ping action arguments.
#[derive(Debug, Clone)]
pub struct PingArgs {
    /// Action hash (keccak256 of the action name)
    pub action: [u8; 32],
    /// Packed topic field — layout depends on action type
    pub topic: [u8; 32],
    /// Additional data (always empty for subspace actions)
    pub data: Vec<u8>,
}

/// Decode ping(bytes32, bytes32, bytes) calldata.
///
/// The calldata is:
/// - 4 bytes: function selector (0xc70d8282)
/// - ABI-encoded (bytes32 action, bytes32 topic, bytes data)
///
/// The `action` field is a keccak256 hash identifying the subspace operation.
/// The `topic` field layout depends on the action type:
///   - Edge actions (verified/related/etc): `bytes32(bytes16)` → target in [0..16], padding in [16..32]
///   - Subspace topic actions (subspace_topic_set/unset): [subspace_id: 16 | topic_id: 16]
pub fn decode_ping_args(calldata: &[u8]) -> Result<PingArgs, DecodeError> {
    // Minimum: 4-byte selector + 2×32 static (bytes32, bytes32) + 32 offset + 32 length = 132 bytes
    if calldata.len() < 132 {
        return Err(DecodeError::DataTooShort {
            expected: 132,
            actual: calldata.len(),
        });
    }

    // Skip the 4-byte selector
    let data = &calldata[4..];

    // Decode (bytes32, bytes32, bytes)
    // Note: abi_decode_params is correct here because the data is raw function
    // parameters (after stripping the 4-byte selector). abi_decode would wrap
    // in an extra single-element tuple, expecting an offset at word 0.
    type PingArgsType = sol! { (bytes32, bytes32, bytes) };
    let (action, topic, ping_data) =
        PingArgsType::abi_decode_params(data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(PingArgs {
        action: action.into(),
        topic: topic.into(),
        data: ping_data.to_vec(),
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
// PROPOSAL_SETTINGS_SELECTED (V2): abi.encode(ProposalParameters).
// Field order matches the Solidity `ProposalParameters` struct:
// (votingMode, partialPct, universalPct, flat, quorum, startDate, lastDate, executeBy).
#[allow(dead_code)] // Used in tests for encoding
type ProposalSettingsUsedDataType =
    sol! { (uint8, uint256, uint256, uint256, uint256, uint256, uint256, uint256) };
// PROPOSAL_VOTED (V2): abi.encode(bytes16 proposalId, uint8 proposalVersion, VoteOption).
type ProposalVotedDataType = sol! { (bytes16, uint8, uint8) };
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

/// Decoded proposal settings from PROPOSAL_SETTINGS_SELECTED (V2 — 8 fields).
/// Field order matches the Solidity `ProposalParameters` struct.
#[derive(Debug, Clone)]
pub struct ProposalSettingsData {
    /// Voting mode (0=Slow, 1=Fast).
    pub voting_mode: u8,
    /// Slow path late execution threshold (0..RATIO_BASE, % of yes/no votes).
    pub partial_percentage_support_threshold: u64,
    /// Slow path early execution threshold (0..RATIO_BASE, % of total editors).
    pub universal_percentage_support_threshold: u64,
    /// Fast path absolute YES votes needed.
    pub flat_support_threshold: u64,
    /// Minimum participating votes for slow path (quorum).
    pub quorum: u64,
    /// Block timestamp when voting starts.
    pub start_date: u64,
    /// Block timestamp when voting ends.
    pub last_date: u64,
    /// Inclusive upper bound timestamp for execution.
    pub execute_by: u64,
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

/// Decode PROPOSAL_SETTINGS_SELECTED data (V2).
///
/// Encoding: `abi.encode(ProposalParameters)` where the struct is
/// `(votingMode, partialPct, universalPct, flat, quorum, startDate, lastDate, executeBy)`.
pub fn decode_proposal_settings_used(data: &[u8]) -> Result<ProposalSettingsData, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::DataTooShort {
            expected: 256, // 8 * 32 bytes
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

    Ok(decoded)
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

fn decode_proposal_settings_used_inner(data: &[u8]) -> Result<ProposalSettingsData, DecodeError> {
    let params = [
        ParamType::Uint(8),   // votingMode
        ParamType::Uint(256), // partialPercentageSupportThreshold
        ParamType::Uint(256), // universalPercentageSupportThreshold
        ParamType::Uint(256), // flatSupportThreshold
        ParamType::Uint(256), // quorum
        ParamType::Uint(256), // startDate
        ParamType::Uint(256), // lastDate
        ParamType::Uint(256), // executeBy
    ];

    let tokens =
        ethabi::decode(&params, data).map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    let read_u8 = |i: usize, name: &str| -> Result<u8, DecodeError> {
        match &tokens[i] {
            Token::Uint(v) => Ok(v.low_u32() as u8),
            _ => Err(DecodeError::AbiDecode(format!("Invalid {name}"))),
        }
    };
    let read_u64 = |i: usize, name: &str| -> Result<u64, DecodeError> {
        match &tokens[i] {
            Token::Uint(v) => Ok(v.as_u64()),
            _ => Err(DecodeError::AbiDecode(format!("Invalid {name}"))),
        }
    };

    Ok(ProposalSettingsData {
        voting_mode: read_u8(0, "voting_mode")?,
        partial_percentage_support_threshold: read_u64(1, "partial_percentage_support_threshold")?,
        universal_percentage_support_threshold: read_u64(
            2,
            "universal_percentage_support_threshold",
        )?,
        flat_support_threshold: read_u64(3, "flat_support_threshold")?,
        quorum: read_u64(4, "quorum")?,
        start_date: read_u64(5, "start_date")?,
        last_date: read_u64(6, "last_date")?,
        execute_by: read_u64(7, "execute_by")?,
    })
}

/// Decoded proposal vote data (V2 — includes proposal version).
#[derive(Debug, Clone)]
pub struct ProposalVotedData {
    /// Proposal ID (16 bytes).
    #[allow(dead_code)] // Available for callers who need it
    pub proposal_id: Vec<u8>,
    /// Proposal version being voted on (uint8 on-chain, monotonically incremented on update).
    pub proposal_version: u8,
    /// Vote option (0=None, 1=Yes, 2=No, 3=Abstain).
    pub vote: u8,
}

/// Decode PROPOSAL_VOTED data (V2).
///
/// Encoding: `abi.encode(bytes16 proposalId, uint8 proposalVersion, VoteOption)`
///
/// The data may arrive wrapped in an ABI `bytes` encoding (offset + length + content)
/// because the SpaceRegistry's Action event emits `_data` as a non-indexed `bytes`
/// parameter. The EVM ABI-encodes this in the log data section, so `log.data()`
/// includes the wrapper. We use `maybe_unwrap_bytes` to strip it, matching the
/// approach used by `decode_proposal_created` and `decode_proposal_settings_used`.
pub fn decode_proposal_voted(data: &[u8]) -> Result<ProposalVotedData, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::DataTooShort {
            expected: 48, // bytes16 + uint8 (padded)
            actual: 0,
        });
    }

    let current = maybe_unwrap_bytes(data);

    let (proposal_id, proposal_version, vote_option) = ProposalVotedDataType::abi_decode(&current)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(ProposalVotedData {
        proposal_id: proposal_id.to_vec(),
        proposal_version,
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
    let data = match data.len() {
        0 => {
            return Err(DecodeError::DataTooShort {
                expected: 160,
                actual: 0,
            });
        }
        160 => &data[64..], // skip headers
        _ => {
            return Err(DecodeError::DataTooLong {
                expected: 160,
                actual: data.len(),
            });
        }
    };

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

/// Decode the topic id from a TOPIC_DECLARED `topic` field.
///
/// Encoding: `bytes32(bytes16(topicId) | padding)`
pub fn decode_topic_declared(topic: &[u8]) -> Result<Vec<u8>, DecodeError> {
    if topic.is_empty() {
        return Ok(vec![0; 16]);
    }

    if topic.len() < 16 {
        return Err(DecodeError::DataTooShort {
            expected: 16,
            actual: topic.len(),
        });
    }

    Ok(topic[..16].to_vec())
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
    use alloy::primitives::{Bytes as PrimBytes, FixedBytes, keccak256};

    #[test]
    fn update_voting_settings_selector_matches_v2_signature() {
        let sig = "updateVotingSettings((uint256,uint256,uint256,uint256,uint256,bool,uint256))";
        let expected: [u8; 4] = keccak256(sig.as_bytes()).0[..4].try_into().unwrap();
        assert_eq!(selectors::UPDATE_VOTING_SETTINGS, expected);
    }

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
    fn test_decode_proposal_voted_v2() {
        let proposal_id = FixedBytes::<16>::from([0xCD; 16]);
        let proposal_version = 7u8;
        let vote_option = 2u8; // No
        let encoded =
            ProposalVotedDataType::abi_encode(&(proposal_id, proposal_version, vote_option));

        let result = decode_proposal_voted(&encoded).unwrap();
        assert_eq!(result.proposal_id, vec![0xCD; 16]);
        assert_eq!(result.proposal_version, 7);
        assert_eq!(result.vote, 2);
    }

    #[test]
    fn decode_voting_settings_args_returns_v2_seven_fields() {
        use alloy::primitives::U256;

        let tuple = (
            U256::from(1_000_000u64), // partial
            U256::from(2_000_000u64), // universal
            U256::from(3u64),         // flat
            U256::from(4u64),         // quorum
            U256::from(5u64),         // duration
            true,                     // disableFastPathAccessForNewMembers
            U256::from(6u64),         // executionGracePeriod
        );
        let tuple_bytes = VotingSettingsTuple::abi_encode(&tuple);

        let mut calldata = Vec::with_capacity(4 + tuple_bytes.len());
        calldata.extend_from_slice(&selectors::UPDATE_VOTING_SETTINGS);
        calldata.extend_from_slice(&tuple_bytes);

        let args = decode_voting_settings_args(&calldata).unwrap();
        assert_eq!(args.partial_percentage_support_threshold, 1_000_000);
        assert_eq!(args.universal_percentage_support_threshold, 2_000_000);
        assert_eq!(args.flat_support_threshold, 3);
        assert_eq!(args.quorum, 4);
        assert_eq!(args.duration, 5);
        assert!(args.disable_fast_path_access_for_new_members);
        assert_eq!(args.execution_grace_period, 6);
    }

    #[test]
    fn decode_voting_settings_data_accepts_raw_event_payload() {
        use alloy::primitives::U256;

        let tuple = (
            U256::from(100u64),
            U256::from(200u64),
            U256::from(300u64),
            U256::from(400u64),
            U256::from(500u64),
            false,
            U256::from(600u64),
        );
        let data = VotingSettingsTuple::abi_encode(&tuple);

        let args = decode_voting_settings_data(&data).unwrap();
        assert_eq!(args.partial_percentage_support_threshold, 100);
        assert_eq!(args.universal_percentage_support_threshold, 200);
        assert_eq!(args.flat_support_threshold, 300);
        assert_eq!(args.quorum, 400);
        assert_eq!(args.duration, 500);
        assert!(!args.disable_fast_path_access_for_new_members);
        assert_eq!(args.execution_grace_period, 600);
    }

    #[test]
    fn decode_voting_settings_data_accepts_bytes_wrapped_payload() {
        use alloy::primitives::U256;
        use alloy::sol_types::SolValue;

        // The EVM may deliver Action.data wrapped in an ABI `bytes` envelope
        // (offset + length + content). Verify the decoder transparently unwraps
        // one level so VOTING_SETTINGS_UPDATED is not silently dropped.
        let tuple = (
            U256::from(1_000_000u64),
            U256::from(2_000_000u64),
            U256::from(3u64),
            U256::from(4u64),
            U256::from(5u64),
            true,
            U256::from(6u64),
        );
        let tuple_bytes = VotingSettingsTuple::abi_encode(&tuple);

        // Wrap tuple encoding inside a `bytes` ABI envelope, mirroring the
        // shape the EVM produces for a non-indexed `bytes` event parameter.
        let wrapped = PrimBytes::from(tuple_bytes).abi_encode();

        let args = decode_voting_settings_data(&wrapped).unwrap();
        assert_eq!(args.partial_percentage_support_threshold, 1_000_000);
        assert_eq!(args.universal_percentage_support_threshold, 2_000_000);
        assert_eq!(args.flat_support_threshold, 3);
        assert_eq!(args.quorum, 4);
        assert_eq!(args.duration, 5);
        assert!(args.disable_fast_path_access_for_new_members);
        assert_eq!(args.execution_grace_period, 6);
    }

    #[test]
    fn decode_voting_settings_data_decodes_raw_tuple_with_small_partial_threshold() {
        use alloy::primitives::U256;

        // Regression test: `maybe_unwrap_bytes` looks at words 0 and 1 of the
        // payload and treats `(offset=32, length<=data.len()-64)` as an ABI
        // `bytes` envelope. For a raw `VotingSettings` tuple, word 0 is
        // `partial_percentage_support_threshold` and word 1 is
        // `universal_percentage_support_threshold`. Choosing `partial == 32`
        // and `universal == 160` produces a valid tuple whose wire layout
        // satisfies the envelope heuristic: the eager pre-unwrap would slice
        // the buffer down to the inner 160 bytes and fail to decode the
        // (now-truncated) 7-word tuple, causing the event to be dropped.
        //
        // With the fix, the raw tuple decode is attempted first and succeeds.
        let tuple = (
            U256::from(32u64),  // partial — matches the "offset" word
            U256::from(160u64), // universal — matches the "length" word (<= 224 - 64)
            U256::from(3u64),
            U256::from(4u64),
            U256::from(5u64),
            true,
            U256::from(6u64),
        );
        let data = VotingSettingsTuple::abi_encode(&tuple);
        assert_eq!(data.len(), 224, "static 7-word tuple should be 224 bytes");

        let args = decode_voting_settings_data(&data).unwrap();
        assert_eq!(args.partial_percentage_support_threshold, 32);
        assert_eq!(args.universal_percentage_support_threshold, 160);
        assert_eq!(args.flat_support_threshold, 3);
        assert_eq!(args.quorum, 4);
        assert_eq!(args.duration, 5);
        assert!(args.disable_fast_path_access_for_new_members);
        assert_eq!(args.execution_grace_period, 6);
    }

    #[test]
    fn decode_proposal_settings_used_returns_v2_eight_fields_with_execute_by() {
        use ethabi::ethereum_types::U256 as EthU256;

        // Encode the V2 ProposalParameters tuple: (votingMode, partialPct,
        // universalPct, flat, quorum, startDate, lastDate, executeBy).
        let encoded = ethabi::encode(&[
            Token::Uint(EthU256::from(1u8)),              // votingMode = Fast (1)
            Token::Uint(EthU256::from(500_000u64)),       // partialPct
            Token::Uint(EthU256::from(750_000u64)),       // universalPct
            Token::Uint(EthU256::from(3u64)),             // flat
            Token::Uint(EthU256::from(10u64)),            // quorum
            Token::Uint(EthU256::from(1_700_000_000u64)), // startDate
            Token::Uint(EthU256::from(1_700_086_400u64)), // lastDate
            Token::Uint(EthU256::from(1_700_691_200u64)), // executeBy
        ]);

        let settings = decode_proposal_settings_used(&encoded).unwrap();
        assert_eq!(settings.voting_mode, 1);
        assert_eq!(settings.partial_percentage_support_threshold, 500_000);
        assert_eq!(settings.universal_percentage_support_threshold, 750_000);
        assert_eq!(settings.flat_support_threshold, 3);
        assert_eq!(settings.quorum, 10);
        assert_eq!(settings.start_date, 1_700_000_000);
        assert_eq!(settings.last_date, 1_700_086_400);
        assert_eq!(settings.execute_by, 1_700_691_200);
    }

    #[test]
    fn test_decode_vote_data_log() {
        let hex_data = "0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000091501554630a41359bee8699b10da8e1000000000000000000000000000000003126d78323f81eb48511bafa39e1200500000000000000000000000000000000";
        let data = hex::decode(&hex_data[2..]).expect("Valid hex string");

        let result = decode_vote_data(&data).unwrap();

        let expected_group_id = hex::decode("91501554630a41359bee8699b10da8e1").unwrap();
        let expected_space_pov = hex::decode("3126d78323f81eb48511bafa39e12005").unwrap();

        assert_eq!(result.version, 0);
        assert_eq!(result.group_id, expected_group_id);
        assert_eq!(result.space_pov, expected_space_pov);
    }

    #[test]
    fn test_decode_vote_data_log_empty_group_id() {
        let hex_data = "0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000060000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003126d78323f81eb48511bafa39e1200500000000000000000000000000000000";
        let data = hex::decode(&hex_data[2..]).expect("Valid hex string");

        let result = decode_vote_data(&data).unwrap();

        assert_eq!(result.version, 0);
        assert_eq!(result.group_id, vec![0u8; 16]);
        let expected_space_pov = hex::decode("3126d78323f81eb48511bafa39e12005").unwrap();
        assert_eq!(result.space_pov, expected_space_pov);
    }

    #[test]
    fn test_decode_vote_data_empty() {
        let result = decode_vote_data(&[]).unwrap_err();
        assert!(matches!(result, DecodeError::DataTooShort { .. }));
    }

    #[test]
    fn test_decode_vote_data_too_long() {
        let data = vec![0u8; 160 + 1];

        let result = decode_vote_data(&data).unwrap_err();
        assert!(matches!(result, DecodeError::DataTooLong { .. }));
    }

    #[test]
    fn test_decode_topic_declared() {
        let mut topic = vec![1u8; 16];
        topic.extend_from_slice(&[0u8; 16]);

        let result = decode_topic_declared(&topic).unwrap();
        assert_eq!(result, vec![1u8; 16]);
    }

    #[test]
    fn test_decode_topic_declared_empty() {
        let result = decode_topic_declared(&[]).unwrap();
        assert_eq!(result, vec![0u8; 16]);
    }

    #[test]
    fn test_decode_topic_declared_short_topic() {
        let result = decode_topic_declared(&[1u8; 15]).unwrap_err();

        match result {
            DecodeError::DataTooShort {
                expected: 16,
                actual: 15,
            } => {}
            other => panic!("unexpected error: {other:?}"),
        }
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

    /// Verify that abi_decode vs abi_decode_params behaves differently for
    /// dynamic tuples but identically for static tuples.
    #[test]
    fn test_abi_decode_vs_decode_params() {
        use alloy::primitives::{Bytes as PrimBytes, U256};
        use alloy::sol_types::SolType;

        // Static tuple: (uint256, uint256, uint256, uint256)
        // abi_decode and abi_decode_params should both work
        type StaticTuple = alloy::sol! { (uint256, uint256, uint256, uint256) };
        let static_encoded = StaticTuple::abi_encode_params(&(
            U256::from(1),
            U256::from(2),
            U256::from(3),
            U256::from(4),
        ));
        assert!(
            StaticTuple::abi_decode(&static_encoded).is_ok(),
            "static: abi_decode should work"
        );
        assert!(
            StaticTuple::abi_decode_params(&static_encoded).is_ok(),
            "static: abi_decode_params should work"
        );

        // Dynamic tuple: (bytes32, bytes32, bytes)
        // abi_decode_params should work, abi_decode should fail
        type DynTuple = alloy::sol! { (bytes32, bytes32, bytes) };
        let dyn_encoded =
            DynTuple::abi_encode_params(&([0xAA_u8; 32], [0xBB_u8; 32], PrimBytes::new()));
        assert!(
            DynTuple::abi_decode(&dyn_encoded).is_err(),
            "dynamic (bytes32,bytes32,bytes): abi_decode should fail on params-encoded data"
        );
        assert!(
            DynTuple::abi_decode_params(&dyn_encoded).is_ok(),
            "dynamic (bytes32,bytes32,bytes): abi_decode_params should work on params-encoded data"
        );

        // Dynamic tuple: (bytes32, bytes, bytes) - like publish
        // Both should behave the same as ping
        type PublishTuple = alloy::sol! { (bytes32, bytes, bytes) };
        let publish_encoded = PublishTuple::abi_encode_params(&(
            [0xAA_u8; 32],
            PrimBytes::from(b"ipfs://QmTest".to_vec()),
            PrimBytes::from(vec![1, 2, 3]),
        ));
        let publish_decode_result = PublishTuple::abi_decode(&publish_encoded);
        let publish_params_result = PublishTuple::abi_decode_params(&publish_encoded);
        println!(
            "publish abi_decode: {}, abi_decode_params: {}",
            if publish_decode_result.is_ok() {
                "OK"
            } else {
                "FAIL"
            },
            if publish_params_result.is_ok() {
                "OK"
            } else {
                "FAIL"
            },
        );
        assert!(
            publish_params_result.is_ok(),
            "publish: abi_decode_params should work"
        );
    }
}
