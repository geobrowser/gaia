//! Integration tests for governance pipeline using mock relay typed builders.
//!
//! These tests verify end-to-end decoding of governance events from the mock relay
//! through the pipeline transformation layer.

use hermes_pipeline::pipelines::governance;
use hermes_pipeline::pipelines::BlockMetadata;
use hermes_relay::source::mock_events::{
    make_id, make_proposal_id, proposal_created, proposal_voted, ProposalAction, VoteOption,
    VotingMode,
};
use hermes_schema::pb::governance::{ProposalActionType, ProposalVoteOption};

fn test_meta() -> BlockMetadata {
    BlockMetadata {
        cursor: "integration_test".to_string(),
        block_number: 100,
        timestamp: "1234567890".to_string(),
    }
}

#[test]
fn test_proposal_created_fast_path_add_member() {
    // Create a fast path proposal to add a member using typed builders
    let space_id = make_id(0x01);
    let proposal_id = make_proposal_id(0xA1);
    let member_address = [0xBB; 20];

    let action = proposal_created(
        space_id,
        proposal_id,
        VotingMode::Fast,
        vec![ProposalAction::add_member(member_address)],
    );

    // Transform through the pipeline
    let result = governance::transform(&[action], &test_meta()).unwrap();

    // Verify the result
    assert_eq!(result.proposals_created.len(), 1);
    let event = &result.proposals_created[0];

    assert_eq!(event.space_id, space_id.to_vec());
    assert_eq!(event.proposal_id, proposal_id.to_vec());
    assert_eq!(
        event.voting_mode,
        hermes_schema::pb::governance::VotingMode::Fast as i32
    );
    assert_eq!(event.action_count, 1);
    assert_eq!(event.action_index, 0);
    assert_eq!(
        event.action_type,
        ProposalActionType::AddMember as i32
    );

    // Verify the decoded target_address
    let proposal_action = event.action.as_ref().unwrap();
    assert_eq!(proposal_action.target_address, member_address.to_vec());
}

#[test]
fn test_proposal_created_fast_path_add_editor() {
    let space_id = make_id(0x02);
    let proposal_id = make_proposal_id(0xA2);
    let editor_address = [0xCC; 20];

    let action = proposal_created(
        space_id,
        proposal_id,
        VotingMode::Fast,
        vec![ProposalAction::add_editor(editor_address)],
    );

    let result = governance::transform(&[action], &test_meta()).unwrap();

    assert_eq!(result.proposals_created.len(), 1);
    let event = &result.proposals_created[0];

    assert_eq!(
        event.action_type,
        ProposalActionType::AddEditor as i32
    );

    let proposal_action = event.action.as_ref().unwrap();
    assert_eq!(proposal_action.target_address, editor_address.to_vec());
}

#[test]
fn test_proposal_created_slow_path_multiple_actions() {
    // Create a slow path proposal with multiple actions
    let space_id = make_id(0x03);
    let proposal_id = make_proposal_id(0xA3);
    let member1 = [0x11; 20];
    let member2 = [0x22; 20];

    let action = proposal_created(
        space_id,
        proposal_id,
        VotingMode::Slow,
        vec![
            ProposalAction::add_member(member1),
            ProposalAction::add_member(member2),
        ],
    );

    let result = governance::transform(&[action], &test_meta()).unwrap();

    // Should emit one event per action
    assert_eq!(result.proposals_created.len(), 2);

    // First action
    let event1 = &result.proposals_created[0];
    assert_eq!(
        event1.voting_mode,
        hermes_schema::pb::governance::VotingMode::Slow as i32
    );
    assert_eq!(event1.action_index, 0);
    assert_eq!(event1.action_count, 2);
    assert_eq!(
        event1.action.as_ref().unwrap().target_address,
        member1.to_vec()
    );

    // Second action
    let event2 = &result.proposals_created[1];
    assert_eq!(event2.action_index, 1);
    assert_eq!(event2.action_count, 2);
    assert_eq!(
        event2.action.as_ref().unwrap().target_address,
        member2.to_vec()
    );
}

#[test]
fn test_proposal_voted_yes() {
    let voter_id = make_id(0x10);
    let space_id = make_id(0x01);
    let proposal_id = make_proposal_id(0xA1);

    let action = proposal_voted(voter_id, space_id, proposal_id, VoteOption::Yes);

    let result = governance::transform(&[action], &test_meta()).unwrap();

    assert_eq!(result.proposals_voted.len(), 1);
    let event = &result.proposals_voted[0];

    assert_eq!(event.voter_id, voter_id.to_vec());
    assert_eq!(event.space_id, space_id.to_vec());
    assert_eq!(event.proposal_id, proposal_id.to_vec());
    assert_eq!(event.vote, ProposalVoteOption::Yes as i32);
}

#[test]
fn test_proposal_voted_no() {
    let voter_id = make_id(0x11);
    let space_id = make_id(0x02);
    let proposal_id = make_proposal_id(0xA2);

    let action = proposal_voted(voter_id, space_id, proposal_id, VoteOption::No);

    let result = governance::transform(&[action], &test_meta()).unwrap();

    assert_eq!(result.proposals_voted.len(), 1);
    assert_eq!(result.proposals_voted[0].vote, ProposalVoteOption::No as i32);
}

#[test]
fn test_proposal_voted_abstain() {
    let voter_id = make_id(0x12);
    let space_id = make_id(0x03);
    let proposal_id = make_proposal_id(0xA3);

    let action = proposal_voted(voter_id, space_id, proposal_id, VoteOption::Abstain);

    let result = governance::transform(&[action], &test_meta()).unwrap();

    assert_eq!(result.proposals_voted.len(), 1);
    assert_eq!(
        result.proposals_voted[0].vote,
        ProposalVoteOption::Abstain as i32
    );
}

#[test]
fn test_full_governance_flow() {
    // Simulate a complete governance flow: create proposal, vote, execute
    use hermes_relay::source::mock_events::proposal_executed;

    let space_id = make_id(0x01);
    let voter1_id = make_id(0x10);
    let voter2_id = make_id(0x11);
    let proposal_id = make_proposal_id(0xB1);
    let new_member = [0xDD; 20];

    let actions = vec![
        // Create proposal
        proposal_created(
            space_id,
            proposal_id,
            VotingMode::Fast,
            vec![ProposalAction::add_member(new_member)],
        ),
        // Vote yes
        proposal_voted(voter1_id, space_id, proposal_id, VoteOption::Yes),
        proposal_voted(voter2_id, space_id, proposal_id, VoteOption::Yes),
        // Execute
        proposal_executed(space_id, proposal_id),
    ];

    let result = governance::transform(&actions, &test_meta()).unwrap();

    assert_eq!(result.proposals_created.len(), 1);
    assert_eq!(result.proposals_voted.len(), 2);
    assert_eq!(result.proposals_executed.len(), 1);

    // Verify the proposal was for adding the correct member
    assert_eq!(
        result.proposals_created[0]
            .action
            .as_ref()
            .unwrap()
            .target_address,
        new_member.to_vec()
    );

    // Verify execution
    assert_eq!(result.proposals_executed[0].space_id, space_id.to_vec());
    assert_eq!(result.proposals_executed[0].proposal_id, proposal_id.to_vec());
}
