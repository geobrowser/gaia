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
// PROPOSAL_CREATED: abi.encode(VotingMode, Action[])
type ProposalCreatedDataType = sol! { (uint8, Action[]) };
// PROPOSAL_VOTED: abi.encode(uint256(proposalId), VoteOption)
type ProposalVotedDataType = sol! { (uint256, uint8) };
type VoteDataType = sol! { (uint16, bytes16, bytes16) };
type EditsPublishedDataType = sol! { (bytes, bytes) };

// ============================================================================
// Governance Decoding
// ============================================================================

/// Decoded proposal creation data.
#[derive(Debug, Clone)]
pub struct ProposalCreatedData {
    /// Voting mode (0=Fast, 1=Slow).
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

/// Decode PROPOSAL_CREATED data.
///
/// Encoding: `abi.encode(VotingMode, Action[])`
pub fn decode_proposal_created(data: &[u8]) -> Result<ProposalCreatedData, DecodeError> {
    if data.is_empty() {
        return Ok(ProposalCreatedData {
            voting_mode: 0,
            actions: vec![],
        });
    }

    let (voting_mode, actions) = ProposalCreatedDataType::abi_decode(data)
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
        voting_mode,
        actions,
    })
}

/// Decoded proposal vote data.
#[derive(Debug, Clone)]
pub struct ProposalVotedData {
    /// Proposal ID (32 bytes).
    pub proposal_id: Vec<u8>,
    /// Vote option (0=Yes, 1=No, 2=Abstain).
    pub vote: u8,
}

/// Decode PROPOSAL_VOTED data.
///
/// Encoding: `abi.encode(uint256(proposalId), VoteOption)`
pub fn decode_proposal_voted(data: &[u8]) -> Result<ProposalVotedData, DecodeError> {
    if data.is_empty() {
        return Err(DecodeError::DataTooShort {
            expected: 64,
            actual: 0,
        });
    }

    let (proposal_id, vote_option) = ProposalVotedDataType::abi_decode(data)
        .map_err(|e| DecodeError::AbiDecode(e.to_string()))?;

    Ok(ProposalVotedData {
        proposal_id: proposal_id.to_be_bytes_vec(),
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
        let actions = vec![Action {
            to: Address::ZERO,
            value: U256::from(1000u64),
            data: vec![1, 2, 3].into(),
        }];
        let voting_mode = 1u8; // Slow
        let encoded = ProposalCreatedDataType::abi_encode(&(voting_mode, actions));

        let result = decode_proposal_created(&encoded).unwrap();
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].to, Address::ZERO.as_slice().to_vec());
        assert_eq!(result.voting_mode, 1);
    }

    #[test]
    fn test_decode_proposal_voted() {
        let proposal_id = U256::from(12345u64);
        let vote_option = 2u8; // No
        let encoded = ProposalVotedDataType::abi_encode(&(proposal_id, vote_option));

        let result = decode_proposal_voted(&encoded).unwrap();
        assert_eq!(result.proposal_id, proposal_id.to_be_bytes_vec());
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
}
