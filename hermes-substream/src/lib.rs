//! Hermes Substream
//!
//! Filters and emits Action events from the Space Registry contract.
//! Provides both raw actions and pre-filtered typed events.

pub mod helpers;
pub mod pb;

// Re-export commonly used helpers
pub use helpers::extract_ipfs_uri;

use pb::hermes::*;
use substreams_ethereum::{block_view::LogView, pb::eth};

// Space Registry proxy contract address (ZC16 testnet)
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [
    0x49, 0x2B, 0xFF, 0x74, 0xb1, 0x3A, 0xCF, 0x3C, 0xC2, 0x49, 0xA9, 0x8d, 0x07, 0x9F, 0x0a, 0x6F,
    0x1d, 0x07, 0xDD, 0x2f,
];

// Action type hashes - keccak256 of action names.
// These are re-exported by `hermes-relay::actions` for consumer-side filtering.
//
// Governance actions use 'GOVERNANCE.' prefix
// Permissionless actions use 'PERMISSIONLESS.' prefix

// GOVERNANCE.SPACE_ID_REGISTERED
pub const ACTION_SPACE_ID_REGISTERED: [u8; 32] = [
    0x72, 0x73, 0x34, 0xed, 0xb8, 0xd5, 0x91, 0xa0, 0xc0, 0x71, 0xa8, 0xa4, 0x20, 0xe3, 0x44, 0xb0,
    0x93, 0xf4, 0x6a, 0x26, 0xe0, 0x47, 0xeb, 0x09, 0x1e, 0xbd, 0xe7, 0x2e, 0xbd, 0x8f, 0x4e, 0xd6,
];
// GOVERNANCE.SPACE_ID_MIGRATED
pub const ACTION_SPACE_ID_MIGRATED: [u8; 32] = [
    0xa7, 0xae, 0xe7, 0xb7, 0xa9, 0x38, 0x5a, 0x27, 0x0f, 0x8d, 0x86, 0x43, 0xc9, 0x3c, 0x42, 0x0d,
    0x97, 0x40, 0x72, 0x0e, 0xb5, 0xce, 0x6f, 0x13, 0x0b, 0x8b, 0x87, 0x41, 0x11, 0x3c, 0xd2, 0x2b,
];
// GOVERNANCE.PROPOSAL_CREATED
pub const ACTION_PROPOSAL_CREATED: [u8; 32] = [
    0xcf, 0x43, 0x56, 0xed, 0x12, 0x6c, 0x00, 0xd2, 0xe5, 0x47, 0xac, 0xe2, 0xf6, 0x99, 0x91, 0xa9,
    0x72, 0xd3, 0x22, 0xb4, 0x53, 0x71, 0xd6, 0x1c, 0xe5, 0x47, 0x8b, 0x1c, 0xb9, 0xac, 0xb4, 0xc2,
];
// GOVERNANCE.PROPOSAL_VOTED
pub const ACTION_PROPOSAL_VOTED: [u8; 32] = [
    0x4e, 0xbf, 0x5f, 0x29, 0x67, 0x6c, 0xed, 0xf7, 0xe2, 0xe4, 0xd3, 0x46, 0xa8, 0x43, 0x32, 0x89,
    0x27, 0x8f, 0x95, 0xa9, 0xfd, 0xa7, 0x36, 0x91, 0xdc, 0x1c, 0xe2, 0x45, 0x74, 0xd5, 0x81, 0x9e,
];
// GOVERNANCE.PROPOSAL_EXECUTED
pub const ACTION_PROPOSAL_EXECUTED: [u8; 32] = [
    0x62, 0xa6, 0x0c, 0x0a, 0x96, 0x81, 0x61, 0x28, 0x71, 0xe0, 0xda, 0xfa, 0x0f, 0x24, 0xbb, 0x0c,
    0x83, 0xcb, 0xdd, 0xe8, 0xbe, 0x5a, 0x62, 0x99, 0x97, 0x9c, 0x88, 0xd3, 0x82, 0x36, 0x9e, 0x96,
];
// GOVERNANCE.EDITOR_ADDED
pub const ACTION_EDITOR_ADDED: [u8; 32] = [
    0x2f, 0x66, 0x58, 0x62, 0xe9, 0x81, 0x27, 0x1c, 0xb9, 0x50, 0xcd, 0x3d, 0xd2, 0xf4, 0x40, 0xde,
    0x3b, 0x71, 0xc4, 0x86, 0x00, 0xbb, 0x28, 0x7a, 0x1a, 0x27, 0x81, 0xce, 0xec, 0x2f, 0x0b, 0x9e,
];
// GOVERNANCE.EDITOR_REMOVED
pub const ACTION_EDITOR_REMOVED: [u8; 32] = [
    0x47, 0xcf, 0x9c, 0xf3, 0x92, 0x90, 0xdb, 0x3a, 0xde, 0xce, 0x73, 0x3a, 0xc4, 0xa9, 0x88, 0x30,
    0xb0, 0x11, 0xa7, 0xed, 0x94, 0x71, 0x45, 0xc0, 0xa2, 0xab, 0x6b, 0x48, 0xab, 0x96, 0x72, 0xd9,
];
// GOVERNANCE.MEMBER_ADDED
pub const ACTION_MEMBER_ADDED: [u8; 32] = [
    0x58, 0xe5, 0x61, 0x15, 0x13, 0x11, 0x6e, 0x7a, 0xa8, 0x3a, 0x0f, 0x41, 0x35, 0x48, 0xf9, 0x90,
    0x21, 0x7f, 0x20, 0xf8, 0xcc, 0xae, 0x64, 0x2f, 0xf8, 0x4b, 0x14, 0xe0, 0xb6, 0x30, 0xd4, 0x89,
];
// GOVERNANCE.MEMBER_REMOVED
pub const ACTION_MEMBER_REMOVED: [u8; 32] = [
    0xf2, 0x3a, 0xe2, 0x52, 0xd6, 0x11, 0x18, 0x59, 0x03, 0xbb, 0xe3, 0xe5, 0x22, 0x9e, 0x4e, 0x3e,
    0x2b, 0x1d, 0x74, 0x85, 0x40, 0x71, 0x67, 0x63, 0x47, 0x9e, 0xe2, 0xd7, 0x82, 0x43, 0xbb, 0xc7,
];
// GOVERNANCE.SPACE_FAST_PATH_RESTRICTED (previously EDITOR_FLAGGED)
pub const ACTION_SPACE_FAST_PATH_RESTRICTED: [u8; 32] = [
    0x9d, 0x04, 0xa4, 0x00, 0xd7, 0x71, 0xd1, 0xd5, 0x21, 0x1f, 0x97, 0xbd, 0x55, 0x7e, 0xdf, 0x5e,
    0x7d, 0x77, 0x71, 0xcb, 0xe7, 0x78, 0x50, 0x34, 0x2f, 0xb8, 0x5e, 0x4f, 0x7b, 0xf9, 0x12, 0x05,
];
// GOVERNANCE.SPACE_FAST_PATH_UNRESTRICTED (previously EDITOR_UNFLAGGED)
pub const ACTION_SPACE_FAST_PATH_UNRESTRICTED: [u8; 32] = [
    0xaf, 0x1c, 0xc7, 0xd5, 0x06, 0x6b, 0x7b, 0x2b, 0xc3, 0x30, 0xdb, 0xa1, 0x35, 0x7b, 0xda, 0x13,
    0x96, 0x01, 0x51, 0x4b, 0xf8, 0x4c, 0x2c, 0x02, 0x69, 0xb8, 0xb9, 0x0f, 0xd8, 0x22, 0xe1, 0x95,
];
// GOVERNANCE.MEMBERSHIP_REQUESTED
pub const ACTION_MEMBERSHIP_REQUESTED: [u8; 32] = [
    0xe0, 0x48, 0xe0, 0xdc, 0x30, 0x1b, 0x1b, 0xb4, 0xe2, 0x44, 0x66, 0x08, 0xd8, 0x85, 0x8e, 0xcf,
    0x95, 0xc3, 0x26, 0xd7, 0x24, 0x1c, 0x99, 0x43, 0xb1, 0x4f, 0x64, 0x7f, 0xd3, 0xa7, 0x8d, 0x9a,
];
// GOVERNANCE.SPACE_LEFT
pub const ACTION_SPACE_LEFT: [u8; 32] = [
    0x13, 0xde, 0x9b, 0xd5, 0x08, 0x68, 0xeb, 0xcf, 0xf7, 0x58, 0xa7, 0xfd, 0xc9, 0x21, 0x4b, 0x3f,
    0xfc, 0xff, 0x05, 0xe3, 0xee, 0x4a, 0x71, 0x65, 0xa6, 0x4c, 0x36, 0x77, 0xe9, 0x05, 0x36, 0xdd,
];
// GOVERNANCE.TOPIC_DECLARED
pub const ACTION_TOPIC_DECLARED: [u8; 32] = [
    0xd0, 0x20, 0xfb, 0xe5, 0xa0, 0x27, 0x0d, 0xab, 0xa9, 0xa9, 0x03, 0x1c, 0x59, 0x40, 0x2c, 0x03,
    0xd5, 0x09, 0x3e, 0xbf, 0xa8, 0xc4, 0xca, 0xa2, 0x20, 0x3c, 0x2c, 0x72, 0xf9, 0x5c, 0xe3, 0x0c,
];
// GOVERNANCE.EDITS_PUBLISHED
pub const ACTION_EDITS_PUBLISHED: [u8; 32] = [
    0x4f, 0xa1, 0x92, 0x15, 0xd8, 0x04, 0x5f, 0xeb, 0xfe, 0x03, 0x18, 0x81, 0x4b, 0xb3, 0x1d, 0x47,
    0x00, 0x93, 0x89, 0xb0, 0x78, 0xae, 0x12, 0xe4, 0x23, 0x0e, 0xce, 0xf6, 0x44, 0xad, 0xc6, 0x5e,
];
// GOVERNANCE.FLAGGED
pub const ACTION_FLAGGED: [u8; 32] = [
    0xbc, 0x48, 0xf0, 0xfa, 0x52, 0x3e, 0x8f, 0xc2, 0x06, 0x5b, 0xee, 0x43, 0x30, 0x4c, 0xad, 0xd5,
    0xc3, 0x9e, 0x2e, 0xa4, 0xb9, 0x17, 0x9c, 0xf5, 0x58, 0x8e, 0xf7, 0x7b, 0xb6, 0xe5, 0xd3, 0x87,
];
// GOVERNANCE.UNFLAGGED
pub const ACTION_UNFLAGGED: [u8; 32] = [
    0xf0, 0xa4, 0x01, 0x28, 0xeb, 0xa9, 0x02, 0x1d, 0x61, 0xfe, 0x51, 0xab, 0xaf, 0xc4, 0xd9, 0x73,
    0x57, 0x8a, 0x60, 0xf1, 0xb2, 0x1a, 0x0b, 0xc8, 0x4e, 0x04, 0x3d, 0x71, 0x5f, 0x4a, 0xb9, 0x79,
];
// GOVERNANCE.SUBSPACE_ADDED
pub const ACTION_SUBSPACE_ADDED: [u8; 32] = [
    0x68, 0x6f, 0x0e, 0x79, 0xd1, 0xe8, 0xa9, 0x5a, 0x9b, 0x23, 0x12, 0xf9, 0x99, 0xaf, 0x06, 0x85,
    0xf7, 0xea, 0x33, 0x2d, 0x38, 0xc8, 0x0f, 0x99, 0x5e, 0x98, 0xa0, 0x53, 0x9a, 0x45, 0x2b, 0xde,
];
// GOVERNANCE.SUBSPACE_REMOVED
pub const ACTION_SUBSPACE_REMOVED: [u8; 32] = [
    0xd4, 0x12, 0x51, 0x00, 0x24, 0xf7, 0x26, 0x2e, 0x19, 0x72, 0x02, 0xe9, 0x84, 0xb0, 0x58, 0xbb,
    0x20, 0x7c, 0x9f, 0xe1, 0xe4, 0x87, 0xb4, 0x9c, 0x60, 0x8d, 0x32, 0x56, 0x9c, 0x2e, 0x9e, 0xbc,
];
// GOVERNANCE.SUBSPACE_VERIFIED
pub const ACTION_SUBSPACE_VERIFIED: [u8; 32] = [
    0xf7, 0x84, 0x31, 0xed, 0xee, 0x20, 0xf4, 0xed, 0xc4, 0x76, 0x6f, 0x7e, 0x4c, 0xd7, 0xee, 0x37,
    0xbf, 0x3d, 0x84, 0x51, 0x4d, 0x93, 0xd8, 0xff, 0x7e, 0x8d, 0x8b, 0x32, 0xdc, 0x8c, 0xcd, 0x39,
];
// GOVERNANCE.SUBSPACE_RELATED
pub const ACTION_SUBSPACE_RELATED: [u8; 32] = [
    0xe1, 0xdf, 0xc5, 0x9a, 0x5f, 0xfb, 0x61, 0x92, 0xbe, 0x6b, 0xd8, 0x24, 0x57, 0xa4, 0x8b, 0x1b,
    0x67, 0x5f, 0x4f, 0xf2, 0x88, 0x6a, 0x6c, 0x14, 0x71, 0xf5, 0xc4, 0x76, 0x31, 0x44, 0x8d, 0xe5,
];
// GOVERNANCE.SUBSPACE_TOPIC_DECLARED
pub const ACTION_SUBSPACE_TOPIC_DECLARED: [u8; 32] = [
    0xf4, 0x75, 0x12, 0x19, 0x47, 0x61, 0x2f, 0x07, 0xc1, 0x38, 0xe5, 0xac, 0x27, 0xaa, 0x31, 0x35,
    0x5a, 0xae, 0xa0, 0xda, 0x30, 0x96, 0xce, 0xa1, 0xb7, 0x02, 0xda, 0xeb, 0x5e, 0x84, 0x77, 0xaa,
];
// PERMISSIONLESS.UPVOTED
pub const ACTION_UPVOTED: [u8; 32] = [
    0x1f, 0xc0, 0x4a, 0x8d, 0x93, 0x87, 0xc7, 0xbd, 0x11, 0x99, 0xa2, 0xa7, 0x7c, 0x8e, 0x53, 0x1a,
    0x7a, 0x7b, 0x11, 0x99, 0x1d, 0xf5, 0xdc, 0xc8, 0xc9, 0xac, 0xb6, 0xab, 0xcb, 0x48, 0x17, 0x25,
];
// PERMISSIONLESS.DOWNVOTED
pub const ACTION_DOWNVOTED: [u8; 32] = [
    0xde, 0x8b, 0x89, 0x7c, 0xe7, 0xcc, 0x54, 0x1d, 0xac, 0xb3, 0x88, 0xd5, 0xaa, 0xbb, 0x3d, 0xc0,
    0xfb, 0x78, 0x56, 0x92, 0x02, 0x84, 0xf4, 0x15, 0x82, 0xc1, 0x5b, 0x5f, 0xc3, 0x1a, 0x86, 0x62,
];
// PERMISSIONLESS.UNVOTED
pub const ACTION_UNVOTED: [u8; 32] = [
    0x3b, 0xd4, 0xc3, 0x37, 0x38, 0x2f, 0x79, 0xaa, 0x50, 0x07, 0xa9, 0x11, 0x69, 0xbb, 0x57, 0x72,
    0x3b, 0x5d, 0xd5, 0x9e, 0x6b, 0x4b, 0xb6, 0x0d, 0x20, 0x36, 0x2b, 0xcc, 0x0d, 0x9d, 0x99, 0x8b,
];
// GOVERNANCE.SPACE_TYPE_DECLARED
pub const ACTION_SPACE_TYPE_DECLARED: [u8; 32] = [
    0x9a, 0x7b, 0x4c, 0x30, 0x36, 0x67, 0xb5, 0x1e, 0x48, 0x76, 0xbb, 0x52, 0xef, 0xe5, 0xb9, 0xa8,
    0x97, 0x5c, 0x1f, 0x31, 0x63, 0x50, 0x16, 0x12, 0x71, 0x16, 0x5c, 0x1d, 0x57, 0x49, 0xf5, 0x66,
];
// GOVERNANCE.SPACE_ID_CLEARED
pub const ACTION_SPACE_ID_CLEARED: [u8; 32] = [
    0x54, 0x7e, 0x17, 0x22, 0x12, 0x81, 0x3c, 0xb3, 0x46, 0xaa, 0x82, 0x43, 0x97, 0xbc, 0x84, 0xc6,
    0x52, 0x39, 0x1f, 0x40, 0x0d, 0x73, 0xb0, 0x67, 0x24, 0x0b, 0x4a, 0x14, 0xd2, 0x10, 0xc4, 0xcb,
];
// GOVERNANCE.PERMISSIONLESS_ACTION_ADDED
pub const ACTION_PERMISSIONLESS_ACTION_ADDED: [u8; 32] = [
    0x7f, 0x72, 0xc0, 0x5a, 0xcc, 0x57, 0x1a, 0xbd, 0x0d, 0x3e, 0x35, 0xec, 0xb3, 0x4f, 0x73, 0xfb,
    0x9d, 0x76, 0xcf, 0xe9, 0x38, 0x95, 0x91, 0x9a, 0xcd, 0xb5, 0xaa, 0x0f, 0x76, 0x03, 0xaa, 0x11,
];
// GOVERNANCE.PERMISSIONLESS_ACTION_REMOVED
pub const ACTION_PERMISSIONLESS_ACTION_REMOVED: [u8; 32] = [
    0x1d, 0x28, 0x31, 0xef, 0xb2, 0x9d, 0x30, 0x38, 0x9d, 0xf0, 0xd6, 0x33, 0xa2, 0x45, 0x1a, 0x89,
    0x74, 0xdd, 0x2b, 0xc4, 0x25, 0x37, 0xca, 0xb0, 0x8e, 0x95, 0xcf, 0x8b, 0x49, 0xcf, 0x78, 0x21,
];
// GOVERNANCE.PROPOSAL_SETTINGS_SELECTED (previously PROPOSAL_SETTINGS_USED)
pub const ACTION_PROPOSAL_SETTINGS_SELECTED: [u8; 32] = [
    0xb3, 0xb3, 0x3d, 0xe1, 0x8e, 0x86, 0x67, 0x15, 0xc2, 0xd6, 0x1e, 0x4d, 0x6f, 0x7a, 0xe2, 0x71,
    0x9e, 0x54, 0x62, 0x1a, 0x74, 0x3e, 0x30, 0xe0, 0x0b, 0x3a, 0xf3, 0x42, 0x64, 0x3d, 0x43, 0x58,
];
// GOVERNANCE.PROPOSAL_UPDATED
pub const ACTION_PROPOSAL_UPDATED: [u8; 32] = [
    0xfb, 0x39, 0xd5, 0xa9, 0xdf, 0xb7, 0x01, 0x3b, 0x16, 0x5f, 0x04, 0x6e, 0xb2, 0xb8, 0x96, 0x5a,
    0x04, 0xd1, 0x8a, 0xf9, 0x40, 0x26, 0xf2, 0x3c, 0xcf, 0xfb, 0x0b, 0xa8, 0xad, 0x22, 0xa5, 0x71,
];

// =============================================================================
// Space Type Constants
// =============================================================================
// These are the keccak256 hashes of space type names, used in SPACE_TYPE_DECLARED
// events to identify the type of space being registered.

// keccak256("DAO_SPACE")
pub const SPACE_TYPE_DAO: [u8; 32] = [
    0xc1, 0x9a, 0xbc, 0xe4, 0xe3, 0xeb, 0x7f, 0x73, 0x6b, 0xb5, 0x76, 0x74, 0x9b, 0x5a, 0x9b, 0xb5,
    0x1c, 0x36, 0x43, 0x49, 0x0d, 0x57, 0x31, 0xd5, 0x76, 0x87, 0xf8, 0xd8, 0x37, 0x62, 0x3c, 0xcb,
];

// keccak256("EOA_SPACE")
pub const SPACE_TYPE_EOA: [u8; 32] = [
    0x52, 0x65, 0xbd, 0x92, 0x01, 0x0f, 0x1b, 0x81, 0xf6, 0xc9, 0x7e, 0xf9, 0xb2, 0x1b, 0x9c, 0xc9,
    0x5f, 0x1c, 0x4c, 0xbb, 0xd2, 0xc9, 0x7c, 0xb7, 0x5b, 0x1f, 0xad, 0x5d, 0xf8, 0x09, 0x6d, 0x87,
];

/// Parse Action event from log.
/// Returns None if not a valid Action event from Space Registry.
fn parse_action(log: LogView) -> Option<Action> {
    if log.address() != SPACE_REGISTRY_ADDRESS {
        return None;
    }

    // The Action event is anonymous with 4 indexed fields
    let topics = log.topics();
    if topics.len() != 4 {
        return None;
    }

    // ZC16: bytes16 values are LEFT-aligned (right-padded with zeros)
    // So space IDs are in bytes 0..16, not 16..32
    Some(Action {
        from_id: topics[0][0..16].to_vec(),
        to_id: topics[1][0..16].to_vec(),
        action: topics[2].to_vec(),
        topic: topics[3].to_vec(),
        data: log.data().to_vec(),
    })
}

// =============================================================================
// Raw Actions
// =============================================================================

#[substreams::handlers::map]
fn map_actions(block: eth::v2::Block) -> Result<Actions, substreams::errors::Error> {
    let actions: Vec<Action> = block.logs().filter_map(|log| parse_action(log)).collect();

    Ok(Actions { actions })
}

// =============================================================================
// Governance Events
// =============================================================================

#[substreams::handlers::map]
fn map_spaces_registered(
    block: eth::v2::Block,
) -> Result<SpaceRegisteredList, substreams::errors::Error> {
    let spaces: Vec<SpaceRegistered> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_ID_REGISTERED)
        .map(|action| SpaceRegistered {
            space_id: action.to_id,
            // ZC16: address is LEFT-aligned (right-padded with zeros)
            space_address: action.topic[0..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SpaceRegisteredList { spaces })
}

#[substreams::handlers::map]
fn map_spaces_migrated(
    block: eth::v2::Block,
) -> Result<SpaceMigratedList, substreams::errors::Error> {
    let spaces: Vec<SpaceMigrated> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_ID_MIGRATED)
        .map(|action| SpaceMigrated {
            space_id: action.from_id,
            // ZC16: address is LEFT-aligned (right-padded with zeros)
            new_space_address: action.topic[0..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SpaceMigratedList { spaces })
}

#[substreams::handlers::map]
fn map_proposals_created(
    block: eth::v2::Block,
) -> Result<ProposalCreatedList, substreams::errors::Error> {
    let proposals: Vec<ProposalCreated> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_PROPOSAL_CREATED)
        .map(|action| ProposalCreated {
            space_id: action.from_id,
            proposal_id: action.topic,
            data: action.data,
        })
        .collect();

    Ok(ProposalCreatedList { proposals })
}

#[substreams::handlers::map]
fn map_proposals_voted(
    block: eth::v2::Block,
) -> Result<ProposalVotedList, substreams::errors::Error> {
    let votes: Vec<ProposalVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_PROPOSAL_VOTED)
        .map(|action| ProposalVoted {
            voter_id: action.from_id,
            space_id: action.to_id,
            proposal_id: action.topic,
            data: action.data,
        })
        .collect();

    Ok(ProposalVotedList { votes })
}

#[substreams::handlers::map]
fn map_proposals_executed(
    block: eth::v2::Block,
) -> Result<ProposalExecutedList, substreams::errors::Error> {
    let proposals: Vec<ProposalExecuted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_PROPOSAL_EXECUTED)
        .map(|action| ProposalExecuted {
            space_id: action.from_id,
            proposal_id: action.topic,
            data: action.data,
        })
        .collect();

    Ok(ProposalExecutedList { proposals })
}

#[substreams::handlers::map]
fn map_editors_added(block: eth::v2::Block) -> Result<EditorAddedList, substreams::errors::Error> {
    let editors: Vec<EditorAdded> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITOR_ADDED)
        .filter(|action| action.data.len() >= 32) // ZC16: address in data field
        .map(|action| EditorAdded {
            space_id: action.from_id,
            // ZC16: address is ABI-encoded in data field (12 bytes padding + 20 bytes address)
            editor_address: action.data[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorAddedList { editors })
}

#[substreams::handlers::map]
fn map_editors_removed(
    block: eth::v2::Block,
) -> Result<EditorRemovedList, substreams::errors::Error> {
    let editors: Vec<EditorRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITOR_REMOVED)
        .filter(|action| action.data.len() >= 32) // ZC16: address in data field
        .map(|action| EditorRemoved {
            space_id: action.from_id,
            // ZC16: address is ABI-encoded in data field (12 bytes padding + 20 bytes address)
            editor_address: action.data[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorRemovedList { editors })
}

#[substreams::handlers::map]
fn map_members_added(block: eth::v2::Block) -> Result<MemberAddedList, substreams::errors::Error> {
    let members: Vec<MemberAdded> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_MEMBER_ADDED)
        .filter(|action| action.data.len() >= 32) // ZC16: address in data field
        .map(|action| MemberAdded {
            space_id: action.from_id,
            // ZC16: address is ABI-encoded in data field (12 bytes padding + 20 bytes address)
            member_address: action.data[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(MemberAddedList { members })
}

#[substreams::handlers::map]
fn map_members_removed(
    block: eth::v2::Block,
) -> Result<MemberRemovedList, substreams::errors::Error> {
    let members: Vec<MemberRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_MEMBER_REMOVED)
        .filter(|action| action.data.len() >= 32) // ZC16: address in data field
        .map(|action| MemberRemoved {
            space_id: action.from_id,
            // ZC16: address is ABI-encoded in data field (12 bytes padding + 20 bytes address)
            member_address: action.data[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(MemberRemovedList { members })
}

#[substreams::handlers::map]
fn map_editors_flagged(
    block: eth::v2::Block,
) -> Result<EditorFlaggedList, substreams::errors::Error> {
    let editors: Vec<EditorFlagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_FAST_PATH_RESTRICTED)
        .map(|action| EditorFlagged {
            space_id: action.from_id,
            // ZC16: address is LEFT-aligned (right-padded with zeros)
            editor_address: action.topic[0..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorFlaggedList { editors })
}

#[substreams::handlers::map]
fn map_editors_unflagged(
    block: eth::v2::Block,
) -> Result<EditorUnflaggedList, substreams::errors::Error> {
    let editors: Vec<EditorUnflagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_FAST_PATH_UNRESTRICTED)
        .map(|action| EditorUnflagged {
            space_id: action.from_id,
            // ZC16: address is LEFT-aligned (right-padded with zeros)
            editor_address: action.topic[0..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorUnflaggedList { editors })
}

#[substreams::handlers::map]
fn map_spaces_left(block: eth::v2::Block) -> Result<SpaceLeftList, substreams::errors::Error> {
    let spaces: Vec<SpaceLeft> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_LEFT)
        .map(|action| SpaceLeft {
            member_id: action.from_id,
            space_id: action.to_id,
            data: action.data,
        })
        .collect();

    Ok(SpaceLeftList { spaces })
}

#[substreams::handlers::map]
fn map_topics_declared(
    block: eth::v2::Block,
) -> Result<TopicDeclaredList, substreams::errors::Error> {
    let topics: Vec<TopicDeclared> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_TOPIC_DECLARED)
        .map(|action| TopicDeclared {
            space_id: action.from_id,
            topic_id: action.topic,
            data: action.data,
        })
        .collect();

    Ok(TopicDeclaredList { topics })
}

#[substreams::handlers::map]
fn map_edits_published(
    block: eth::v2::Block,
) -> Result<EditsPublishedList, substreams::errors::Error> {
    let edits: Vec<EditsPublished> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITS_PUBLISHED)
        .map(|action| {
            let content_uri = helpers::extract_ipfs_uri(&action.data).unwrap_or_default();
            EditsPublished {
                space_id: action.from_id,
                data: action.data,
                content_uri,
            }
        })
        .collect();

    Ok(EditsPublishedList { edits })
}

#[substreams::handlers::map]
fn map_flagged(block: eth::v2::Block) -> Result<FlaggedList, substreams::errors::Error> {
    let flags: Vec<Flagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_FLAGGED)
        .map(|action| Flagged {
            flagger_id: action.from_id,
            space_id: action.to_id,
            data: action.data,
        })
        .collect();

    Ok(FlaggedList { flags })
}

#[substreams::handlers::map]
fn map_unflagged(block: eth::v2::Block) -> Result<UnflaggedList, substreams::errors::Error> {
    let unflags: Vec<Unflagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_UNFLAGGED)
        .map(|action| Unflagged {
            unflagger_id: action.from_id,
            space_id: action.to_id,
            data: action.data,
        })
        .collect();

    Ok(UnflaggedList { unflags })
}

#[substreams::handlers::map]
fn map_subspaces_removed(
    block: eth::v2::Block,
) -> Result<SubspaceRemovedList, substreams::errors::Error> {
    let subspaces: Vec<SubspaceRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_REMOVED)
        .map(|action| SubspaceRemoved {
            parent_space_id: action.from_id,
            // ZC16: bytes16 is LEFT-aligned (right-padded with zeros)
            subspace_id: action.topic[0..16].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceRemovedList { subspaces })
}

#[substreams::handlers::map]
fn map_subspaces_verified(
    block: eth::v2::Block,
) -> Result<SubspaceVerifiedList, substreams::errors::Error> {
    let subspaces: Vec<SubspaceVerified> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_VERIFIED)
        .map(|action| SubspaceVerified {
            parent_space_id: action.from_id,
            // ZC16: bytes16 is LEFT-aligned (right-padded with zeros)
            subspace_id: action.topic[0..16].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceVerifiedList { subspaces })
}

#[substreams::handlers::map]
fn map_subspaces_related(
    block: eth::v2::Block,
) -> Result<SubspaceRelatedList, substreams::errors::Error> {
    let subspaces: Vec<SubspaceRelated> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_RELATED)
        .map(|action| SubspaceRelated {
            parent_space_id: action.from_id,
            // ZC16: bytes16 is LEFT-aligned (right-padded with zeros)
            subspace_id: action.topic[0..16].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceRelatedList { subspaces })
}

#[substreams::handlers::map]
fn map_subspaces_topic_declared(
    block: eth::v2::Block,
) -> Result<SubspaceTopicDeclaredList, substreams::errors::Error> {
    let declarations: Vec<SubspaceTopicDeclared> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_TOPIC_DECLARED)
        .map(|action| SubspaceTopicDeclared {
            parent_space_id: action.from_id,
            subspace_id: action.topic[0..16].to_vec(),
            topic_id: action.topic[16..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceTopicDeclaredList { declarations })
}

#[substreams::handlers::map]
fn map_space_types_declared(
    block: eth::v2::Block,
) -> Result<SpaceTypeDeclaredList, substreams::errors::Error> {
    let declarations: Vec<SpaceTypeDeclared> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_TYPE_DECLARED)
        .map(|action| SpaceTypeDeclared {
            space_id: action.from_id, // from_id == to_id for this action
            space_type: action.topic,
            version: action.data,
        })
        .collect();

    Ok(SpaceTypeDeclaredList { declarations })
}

#[substreams::handlers::map]
fn map_spaces_cleared(
    block: eth::v2::Block,
) -> Result<SpaceClearedList, substreams::errors::Error> {
    let spaces: Vec<SpaceCleared> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_ID_CLEARED)
        .map(|action| SpaceCleared {
            space_id: action.from_id,
            // ZC16: address is LEFT-aligned (right-padded with zeros)
            space_address: action.topic[0..20].to_vec(),
        })
        .collect();

    Ok(SpaceClearedList { spaces })
}

#[substreams::handlers::map]
fn map_proposal_settings_used(
    block: eth::v2::Block,
) -> Result<ProposalSettingsUsedList, substreams::errors::Error> {
    let settings: Vec<ProposalSettingsUsed> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_PROPOSAL_SETTINGS_SELECTED)
        .map(|action| ProposalSettingsUsed {
            space_id: action.from_id,
            // ZC16: bytes16 is LEFT-aligned (right-padded with zeros)
            proposal_id: action.topic[0..16].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ProposalSettingsUsedList { settings })
}

#[substreams::handlers::map]
fn map_proposals_updated(
    block: eth::v2::Block,
) -> Result<ProposalUpdatedList, substreams::errors::Error> {
    let proposals: Vec<ProposalUpdated> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_PROPOSAL_UPDATED)
        .map(|action| ProposalUpdated {
            space_id: action.from_id,
            // ZC16: bytes16 is LEFT-aligned (right-padded with zeros)
            proposal_id: action.topic[0..16].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ProposalUpdatedList { proposals })
}

// =============================================================================
// Permissionless Events
// =============================================================================

#[substreams::handlers::map]
fn map_objects_upvoted(
    block: eth::v2::Block,
) -> Result<ObjectUpvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_UPVOTED)
        .map(|action| ObjectVoted {
            voter_id: action.from_id,
            object_type: action.topic[0..4].to_vec(),
            object_id: action.topic[4..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ObjectUpvotedList { votes })
}

#[substreams::handlers::map]
fn map_objects_downvoted(
    block: eth::v2::Block,
) -> Result<ObjectDownvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_DOWNVOTED)
        .map(|action| ObjectVoted {
            voter_id: action.from_id,
            object_type: action.topic[0..4].to_vec(),
            object_id: action.topic[4..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ObjectDownvotedList { votes })
}

#[substreams::handlers::map]
fn map_objects_unvoted(
    block: eth::v2::Block,
) -> Result<ObjectUnvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_UNVOTED)
        .map(|action| ObjectVoted {
            voter_id: action.from_id,
            object_type: action.topic[0..4].to_vec(),
            object_id: action.topic[4..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ObjectUnvotedList { votes })
}
