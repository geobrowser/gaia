//! Hermes Substream
//!
//! Filters and emits Action events from the Space Registry contract.
//! Provides both raw actions and pre-filtered typed events.

pub mod helpers;
mod pb;

use pb::hermes::*;
use substreams_ethereum::{block_view::LogView, pb::eth};

// TODO: Replace with actual Space Registry contract address
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [0u8; 20];

// Action type hashes - keccak256 of action names
const ACTION_SPACE_ID_REGISTERED: [u8; 32] = [0x72, 0x73, 0x34, 0xed, 0xb8, 0xd5, 0x91, 0xa0, 0xc0, 0x71, 0xa8, 0xa4, 0x20, 0xe3, 0x44, 0xb0, 0x93, 0xf4, 0x6a, 0x26, 0xe0, 0x47, 0xeb, 0x09, 0x1e, 0xbd, 0xe7, 0x2e, 0xbd, 0x8f, 0x4e, 0xd6];
const ACTION_SPACE_ID_MIGRATED: [u8; 32] = [0xa7, 0xae, 0xe7, 0xb7, 0xa9, 0x38, 0x5a, 0x27, 0x0f, 0x8d, 0x86, 0x43, 0xc9, 0x3c, 0x42, 0x0d, 0x97, 0x40, 0x72, 0x0e, 0xb5, 0xce, 0x6f, 0x13, 0x0b, 0x8b, 0x87, 0x41, 0x11, 0x3c, 0xd2, 0x2b];
const ACTION_PROPOSAL_CREATED: [u8; 32] = [0xcf, 0x43, 0x56, 0xed, 0x12, 0x6c, 0x00, 0xd2, 0xe5, 0x47, 0xac, 0xe2, 0xf6, 0x99, 0x91, 0xa9, 0x72, 0xd3, 0x22, 0xb4, 0x53, 0x71, 0xd6, 0x1c, 0xe5, 0x47, 0x8b, 0x1c, 0xb9, 0xac, 0xb4, 0xc2];
const ACTION_PROPOSAL_VOTED: [u8; 32] = [0x4e, 0xbf, 0x5f, 0x29, 0x67, 0x6c, 0xed, 0xf7, 0xe2, 0xe4, 0xd3, 0x46, 0xa8, 0x43, 0x32, 0x89, 0x27, 0x8f, 0x95, 0xa9, 0xfd, 0xa7, 0x36, 0x91, 0xdc, 0x1c, 0xe2, 0x45, 0x74, 0xd5, 0x81, 0x9e];
const ACTION_PROPOSAL_EXECUTED: [u8; 32] = [0x62, 0xa6, 0x0c, 0x0a, 0x96, 0x81, 0x61, 0x28, 0x71, 0xe0, 0xda, 0xfa, 0x0f, 0x24, 0xbb, 0x0c, 0x83, 0xcb, 0xdd, 0xe8, 0xbe, 0x5a, 0x62, 0x99, 0x97, 0x9c, 0x88, 0xd3, 0x82, 0x36, 0x9e, 0x96];
const ACTION_EDITOR_ADDED: [u8; 32] = [0x2f, 0x66, 0x58, 0x62, 0xe9, 0x81, 0x27, 0x1c, 0xb9, 0x50, 0xcd, 0x3d, 0xd2, 0xf4, 0x40, 0xde, 0x3b, 0x71, 0xc4, 0x86, 0x00, 0xbb, 0x28, 0x7a, 0x1a, 0x27, 0x81, 0xce, 0xec, 0x2f, 0x0b, 0x9e];
const ACTION_EDITOR_REMOVED: [u8; 32] = [0x47, 0xcf, 0x9c, 0xf3, 0x92, 0x90, 0xdb, 0x3a, 0xde, 0xce, 0x73, 0x3a, 0xc4, 0xa9, 0x88, 0x30, 0xb0, 0x11, 0xa7, 0xed, 0x94, 0x71, 0x45, 0xc0, 0xa2, 0xab, 0x6b, 0x48, 0xab, 0x96, 0x72, 0xd9];
const ACTION_MEMBER_ADDED: [u8; 32] = [0x58, 0xe5, 0x61, 0x15, 0x13, 0x11, 0x6e, 0x7a, 0xa8, 0x3a, 0x0f, 0x41, 0x35, 0x48, 0xf9, 0x90, 0x21, 0x7f, 0x20, 0xf8, 0xcc, 0xae, 0x64, 0x2f, 0xf8, 0x4b, 0x14, 0xe0, 0xb6, 0x30, 0xd4, 0x89];
const ACTION_MEMBER_REMOVED: [u8; 32] = [0xf2, 0x3a, 0xe2, 0x52, 0xd6, 0x11, 0x18, 0x59, 0x03, 0xbb, 0xe3, 0xe5, 0x22, 0x9e, 0x4e, 0x3e, 0x2b, 0x1d, 0x74, 0x85, 0x40, 0x71, 0x67, 0x63, 0x47, 0x9e, 0xe2, 0xd7, 0x82, 0x43, 0xbb, 0xc7];
const ACTION_EDITOR_FLAGGED: [u8; 32] = [0xbb, 0x1a, 0x16, 0xb8, 0xbd, 0x0a, 0x30, 0xa1, 0x94, 0xf4, 0xd7, 0x24, 0x7a, 0x82, 0x76, 0x57, 0x32, 0xd2, 0xd7, 0x29, 0xd8, 0xa0, 0xa8, 0x23, 0x1d, 0x91, 0x68, 0xbd, 0x41, 0x2c, 0x6b, 0x0e];
const ACTION_EDITOR_UNFLAGGED: [u8; 32] = [0xf2, 0xdb, 0x16, 0x7b, 0xcd, 0x11, 0xbc, 0x0a, 0xf7, 0xf7, 0x89, 0xd0, 0x25, 0x7d, 0x3c, 0x13, 0x19, 0x30, 0x2a, 0xab, 0xa8, 0x2e, 0xe2, 0xa2, 0xd8, 0x30, 0xf8, 0xa4, 0x84, 0xc6, 0x2d, 0x2f];
const ACTION_SPACE_LEFT: [u8; 32] = [0x13, 0xde, 0x9b, 0xd5, 0x08, 0x68, 0xeb, 0xcf, 0xf7, 0x58, 0xa7, 0xfd, 0xc9, 0x21, 0x4b, 0x3f, 0xfc, 0xff, 0x05, 0xe3, 0xee, 0x4a, 0x71, 0x65, 0xa6, 0x4c, 0x36, 0x77, 0xe9, 0x05, 0x36, 0xdd];
const ACTION_TOPIC_DECLARED: [u8; 32] = [0xd0, 0x20, 0xfb, 0xe5, 0xa0, 0x27, 0x0d, 0xab, 0xa9, 0xa9, 0x03, 0x1c, 0x59, 0x40, 0x2c, 0x03, 0xd5, 0x09, 0x3e, 0xbf, 0xa8, 0xc4, 0xca, 0xa2, 0x20, 0x3c, 0x2c, 0x72, 0xf9, 0x5c, 0xe3, 0x0c];
const ACTION_EDITS_PUBLISHED: [u8; 32] = [0x4f, 0xa1, 0x92, 0x15, 0xd8, 0x04, 0x5f, 0xeb, 0xfe, 0x03, 0x18, 0x81, 0x4b, 0xb3, 0x1d, 0x47, 0x00, 0x93, 0x89, 0xb0, 0x78, 0xae, 0x12, 0xe4, 0x23, 0x0e, 0xce, 0xf6, 0x44, 0xad, 0xc6, 0x5e];
const ACTION_FLAGGED: [u8; 32] = [0xbc, 0x48, 0xf0, 0xfa, 0x52, 0x3e, 0x8f, 0xc2, 0x06, 0x5b, 0xee, 0x43, 0x30, 0x4c, 0xad, 0xd5, 0xc3, 0x9e, 0x2e, 0xa4, 0xb9, 0x17, 0x9c, 0xf5, 0x58, 0x8e, 0xf7, 0x7b, 0xb6, 0xe5, 0xd3, 0x87];
const ACTION_SUBSPACE_ADDED: [u8; 32] = [0x68, 0x6f, 0x0e, 0x79, 0xd1, 0xe8, 0xa9, 0x5a, 0x9b, 0x23, 0x12, 0xf9, 0x99, 0xaf, 0x06, 0x85, 0xf7, 0xea, 0x33, 0x2d, 0x38, 0xc8, 0x0f, 0x99, 0x5e, 0x98, 0xa0, 0x53, 0x9a, 0x45, 0x2b, 0xde];
const ACTION_SUBSPACE_REMOVED: [u8; 32] = [0xd4, 0x12, 0x51, 0x00, 0x24, 0xf7, 0x26, 0x2e, 0x19, 0x72, 0x02, 0xe9, 0x84, 0xb0, 0x58, 0xbb, 0x20, 0x7c, 0x9f, 0xe1, 0xe4, 0x87, 0xb4, 0x9c, 0x60, 0x8d, 0x32, 0x56, 0x9c, 0x2e, 0x9e, 0xbc];
const ACTION_OBJECT_UPVOTED: [u8; 32] = [0x2c, 0xdd, 0x99, 0xb8, 0xdb, 0xde, 0x97, 0x5f, 0x05, 0x17, 0x8e, 0x23, 0x92, 0xb5, 0x63, 0xb8, 0x84, 0xc9, 0x58, 0xaf, 0x0e, 0x7c, 0x3b, 0x5a, 0x79, 0x85, 0xf8, 0xd0, 0x09, 0xfa, 0x41, 0x95];
const ACTION_OBJECT_DOWNVOTED: [u8; 32] = [0xb0, 0x6b, 0x60, 0xe1, 0x1f, 0x65, 0x13, 0xf3, 0xdf, 0x5a, 0x5e, 0x0b, 0x09, 0x5a, 0xec, 0xc4, 0x0f, 0xae, 0x02, 0xb1, 0x85, 0x63, 0xc6, 0x11, 0xf1, 0x0a, 0xb5, 0xed, 0x9d, 0x9b, 0xb7, 0x15];
const ACTION_OBJECT_UNVOTED: [u8; 32] = [0xab, 0xa4, 0x9c, 0x6d, 0xa7, 0x70, 0x58, 0x8e, 0xd6, 0x02, 0x5f, 0x73, 0x6d, 0xa8, 0x76, 0xb7, 0x3b, 0xc0, 0xc7, 0xdc, 0xfd, 0xcd, 0x27, 0x5f, 0xb4, 0x31, 0x6e, 0x8b, 0xf2, 0x25, 0xc1, 0x83];

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

    Some(Action {
        from_id: topics[0][16..32].to_vec(),
        to_id: topics[1][16..32].to_vec(),
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
    let actions: Vec<Action> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .collect();

    Ok(Actions { actions })
}

// =============================================================================
// Governance Events
// =============================================================================

#[substreams::handlers::map]
fn map_spaces_registered(block: eth::v2::Block) -> Result<SpaceRegisteredList, substreams::errors::Error> {
    let spaces: Vec<SpaceRegistered> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_ID_REGISTERED)
        .map(|action| SpaceRegistered {
            space_id: action.from_id,
            space_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SpaceRegisteredList { spaces })
}

#[substreams::handlers::map]
fn map_spaces_migrated(block: eth::v2::Block) -> Result<SpaceMigratedList, substreams::errors::Error> {
    let spaces: Vec<SpaceMigrated> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SPACE_ID_MIGRATED)
        .map(|action| SpaceMigrated {
            space_id: action.from_id,
            new_space_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SpaceMigratedList { spaces })
}

#[substreams::handlers::map]
fn map_proposals_created(block: eth::v2::Block) -> Result<ProposalCreatedList, substreams::errors::Error> {
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
fn map_proposals_voted(block: eth::v2::Block) -> Result<ProposalVotedList, substreams::errors::Error> {
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
fn map_proposals_executed(block: eth::v2::Block) -> Result<ProposalExecutedList, substreams::errors::Error> {
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
        .map(|action| EditorAdded {
            space_id: action.from_id,
            editor_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorAddedList { editors })
}

#[substreams::handlers::map]
fn map_editors_removed(block: eth::v2::Block) -> Result<EditorRemovedList, substreams::errors::Error> {
    let editors: Vec<EditorRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITOR_REMOVED)
        .map(|action| EditorRemoved {
            space_id: action.from_id,
            editor_address: action.topic[12..32].to_vec(),
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
        .map(|action| MemberAdded {
            space_id: action.from_id,
            member_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(MemberAddedList { members })
}

#[substreams::handlers::map]
fn map_members_removed(block: eth::v2::Block) -> Result<MemberRemovedList, substreams::errors::Error> {
    let members: Vec<MemberRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_MEMBER_REMOVED)
        .map(|action| MemberRemoved {
            space_id: action.from_id,
            member_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(MemberRemovedList { members })
}

#[substreams::handlers::map]
fn map_editors_flagged(block: eth::v2::Block) -> Result<EditorFlaggedList, substreams::errors::Error> {
    let editors: Vec<EditorFlagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITOR_FLAGGED)
        .map(|action| EditorFlagged {
            space_id: action.from_id,
            editor_address: action.topic[12..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(EditorFlaggedList { editors })
}

#[substreams::handlers::map]
fn map_editors_unflagged(block: eth::v2::Block) -> Result<EditorUnflaggedList, substreams::errors::Error> {
    let editors: Vec<EditorUnflagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITOR_UNFLAGGED)
        .map(|action| EditorUnflagged {
            space_id: action.from_id,
            editor_address: action.topic[12..32].to_vec(),
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
fn map_topics_declared(block: eth::v2::Block) -> Result<TopicDeclaredList, substreams::errors::Error> {
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
fn map_edits_published(block: eth::v2::Block) -> Result<EditsPublishedList, substreams::errors::Error> {
    let edits: Vec<EditsPublished> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_EDITS_PUBLISHED)
        .map(|action| EditsPublished {
            space_id: action.from_id,
            data: action.data,
        })
        .collect();

    Ok(EditsPublishedList { edits })
}

#[substreams::handlers::map]
fn map_content_flagged(block: eth::v2::Block) -> Result<ContentFlaggedList, substreams::errors::Error> {
    let flags: Vec<ContentFlagged> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_FLAGGED)
        .map(|action| ContentFlagged {
            flagger_id: action.from_id,
            space_id: action.to_id,
            data: action.data,
        })
        .collect();

    Ok(ContentFlaggedList { flags })
}

#[substreams::handlers::map]
fn map_subspaces_added(block: eth::v2::Block) -> Result<SubspaceAddedList, substreams::errors::Error> {
    let subspaces: Vec<SubspaceAdded> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_ADDED)
        .map(|action| SubspaceAdded {
            parent_space_id: action.from_id,
            subspace_id: action.topic[16..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceAddedList { subspaces })
}

#[substreams::handlers::map]
fn map_subspaces_removed(block: eth::v2::Block) -> Result<SubspaceRemovedList, substreams::errors::Error> {
    let subspaces: Vec<SubspaceRemoved> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_SUBSPACE_REMOVED)
        .map(|action| SubspaceRemoved {
            parent_space_id: action.from_id,
            subspace_id: action.topic[16..32].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(SubspaceRemovedList { subspaces })
}

// =============================================================================
// Permissionless Events
// =============================================================================

#[substreams::handlers::map]
fn map_objects_upvoted(block: eth::v2::Block) -> Result<ObjectUpvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_OBJECT_UPVOTED)
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
fn map_objects_downvoted(block: eth::v2::Block) -> Result<ObjectDownvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_OBJECT_DOWNVOTED)
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
fn map_objects_unvoted(block: eth::v2::Block) -> Result<ObjectUnvotedList, substreams::errors::Error> {
    let votes: Vec<ObjectVoted> = block
        .logs()
        .filter_map(|log| parse_action(log))
        .filter(|action| action.action.as_slice() == ACTION_OBJECT_UNVOTED)
        .map(|action| ObjectVoted {
            voter_id: action.from_id,
            object_type: action.topic[0..4].to_vec(),
            object_id: action.topic[4..20].to_vec(),
            data: action.data,
        })
        .collect();

    Ok(ObjectUnvotedList { votes })
}
