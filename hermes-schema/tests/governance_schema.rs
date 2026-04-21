use hermes_schema::pb::governance;
use prost::Message;

#[test]
fn hermes_voting_settings_updated_roundtrips_all_fields() {
    let msg = governance::HermesVotingSettingsUpdated {
        space_id: vec![0xAB; 16],
        partial_percentage_support_threshold: 1_000_000,
        universal_percentage_support_threshold: 2_000_000,
        flat_support_threshold: 3,
        quorum: 4,
        duration: 5,
        disable_fast_path_access_for_new_members: true,
        execution_grace_period: 6,
        meta: None,
    };

    let bytes = msg.encode_to_vec();
    let decoded = governance::HermesVotingSettingsUpdated::decode(&*bytes).unwrap();

    assert_eq!(msg, decoded);
}

#[test]
fn hermes_proposal_voted_carries_proposal_version() {
    let msg = governance::HermesProposalVoted {
        voter_id: vec![0x11; 16],
        space_id: vec![0x22; 16],
        proposal_id: vec![0x33; 16],
        vote: governance::ProposalVoteOption::VoteOptionYes as i32,
        meta: None,
        proposal_version: 7,
    };

    let bytes = msg.encode_to_vec();
    let decoded = governance::HermesProposalVoted::decode(&*bytes).unwrap();

    assert_eq!(msg, decoded);
    assert_eq!(decoded.proposal_version, 7);
}

#[test]
fn update_voting_settings_action_has_v2_seven_fields() {
    let msg = governance::UpdateVotingSettingsAction {
        partial_percentage_support_threshold: 1,
        universal_percentage_support_threshold: 2,
        flat_support_threshold: 3,
        quorum: 4,
        duration: 5,
        disable_fast_path_access_for_new_members: true,
        execution_grace_period: 6,
    };

    let bytes = msg.encode_to_vec();
    let decoded = governance::UpdateVotingSettingsAction::decode(&*bytes).unwrap();

    assert_eq!(msg, decoded);
}

#[test]
fn proposal_settings_has_v2_eight_fields_with_execute_by() {
    let msg = governance::ProposalSettings {
        voting_mode: governance::VotingMode::Slow as i32,
        partial_percentage_support_threshold: 500_000,
        universal_percentage_support_threshold: 750_000,
        flat_support_threshold: 3,
        quorum: 10,
        start_date: 1_700_000_000,
        last_date: 1_700_086_400,
        execute_by: 1_700_691_200,
    };

    let bytes = msg.encode_to_vec();
    let decoded = governance::ProposalSettings::decode(&*bytes).unwrap();

    assert_eq!(msg, decoded);
    assert_eq!(decoded.execute_by, 1_700_691_200);
}
