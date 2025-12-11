//! Hermes Substream
//!
//! Filters and emits Action events from the Space Registry contract.
//! Provides both raw actions and pre-filtered typed events.

pub mod helpers;
mod pb;

use pb::hermes::*;
use substreams_ethereum::pb::eth;

// TODO: Replace with actual Space Registry contract address
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [0u8; 20];

// Action type hashes - keccak256 of action names
// TODO: Compute actual hashes
const ACTION_SPACE_ID_REGISTERED: [u8; 32] = [0u8; 32];
const ACTION_SPACE_ID_MIGRATED: [u8; 32] = [0u8; 32];
const ACTION_PROPOSAL_CREATED: [u8; 32] = [0u8; 32];
const ACTION_PROPOSAL_VOTED: [u8; 32] = [0u8; 32];
const ACTION_PROPOSAL_EXECUTED: [u8; 32] = [0u8; 32];
const ACTION_EDITOR_ADDED: [u8; 32] = [0u8; 32];
const ACTION_EDITOR_REMOVED: [u8; 32] = [0u8; 32];
const ACTION_MEMBER_ADDED: [u8; 32] = [0u8; 32];
const ACTION_MEMBER_REMOVED: [u8; 32] = [0u8; 32];
const ACTION_EDITOR_FLAGGED: [u8; 32] = [0u8; 32];
const ACTION_EDITOR_UNFLAGGED: [u8; 32] = [0u8; 32];
const ACTION_SPACE_LEFT: [u8; 32] = [0u8; 32];
const ACTION_TOPIC_DECLARED: [u8; 32] = [0u8; 32];
const ACTION_EDITS_PUBLISHED: [u8; 32] = [0u8; 32];
const ACTION_FLAGGED: [u8; 32] = [0u8; 32];
const ACTION_SUBSPACE_ADDED: [u8; 32] = [0u8; 32];
const ACTION_SUBSPACE_REMOVED: [u8; 32] = [0u8; 32];
const ACTION_OBJECT_UPVOTED: [u8; 32] = [0u8; 32];
const ACTION_OBJECT_DOWNVOTED: [u8; 32] = [0u8; 32];
const ACTION_OBJECT_UNVOTED: [u8; 32] = [0u8; 32];

/// Parse Action event from log topics.
/// Returns None if not a valid Action event from Space Registry.
fn parse_action(log: &eth::v2::Log) -> Option<Action> {
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
        .filter_map(|log| parse_action(&log))
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
