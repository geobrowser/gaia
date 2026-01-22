//! Benchmarks for pipeline transformation functions.
//!
//! These benchmarks measure the performance of transforming raw blockchain actions
//! into typed Hermes protobuf events ready for Kafka.

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::SolType;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hermes_codec::actions;
use hermes_pipeline::pipelines::{self, BlockMetadata};
use hermes_relay::Action;

// Solidity type for encoding test data
sol! {
    struct TestAction {
        address to;
        uint256 value;
        bytes data;
    }
}
type ProposalCreatedDataType = sol! { (uint8, TestAction[]) };
type ProposalVotedDataType = sol! { (uint256, uint8) };
type VoteDataType = sol! { (uint16, bytes16, bytes16) };

/// Create test block metadata.
fn test_meta() -> BlockMetadata {
    BlockMetadata {
        cursor: "bench_cursor_12345".to_string(),
        block_number: 1000000,
        timestamp: "1700000000".to_string(),
    }
}

/// Generate a proposal created action with the specified number of proposal actions.
fn make_proposal_created_action(space_id: [u8; 16], num_actions: usize) -> Action {
    let proposal_actions: Vec<TestAction> = (0..num_actions)
        .map(|i| {
            let mut calldata = vec![0xca, 0x6d, 0x56, 0xdc]; // ADD_MEMBER selector
            calldata.extend_from_slice(&[0u8; 12]);
            calldata.extend_from_slice(&[(i % 256) as u8; 20]);
            TestAction {
                to: Address::ZERO,
                value: U256::ZERO,
                data: calldata.into(),
            }
        })
        .collect();

    let encoded = ProposalCreatedDataType::abi_encode(&(0u8, proposal_actions));

    Action {
        from_id: space_id.to_vec(),
        to_id: vec![],
        action: actions::PROPOSAL_CREATED.to_vec(),
        topic: vec![0xAA; 32], // proposal_id
        data: encoded,
    }
}

/// Generate a proposal voted action.
fn make_proposal_voted_action(voter_id: [u8; 16], space_id: [u8; 16]) -> Action {
    let proposal_id = U256::from(12345u64);
    let vote_option = 1u8; // Yes
    let encoded = ProposalVotedDataType::abi_encode(&(proposal_id, vote_option));

    Action {
        from_id: voter_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::PROPOSAL_VOTED.to_vec(),
        topic: vec![0xBB; 32], // proposal_id
        data: encoded,
    }
}

/// Generate a proposal executed action.
fn make_proposal_executed_action(space_id: [u8; 16]) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![],
        action: actions::PROPOSAL_EXECUTED.to_vec(),
        topic: vec![0xCC; 32], // proposal_id
        data: vec![],
    }
}

/// Generate an upvote action.
fn make_upvote_action(voter_id: [u8; 16]) -> Action {
    let version = 1u16;
    let group_id = FixedBytes::<16>::from([0xAAu8; 16]);
    let space_pov = FixedBytes::<16>::from([0xBBu8; 16]);
    let encoded = VoteDataType::abi_encode(&(version, group_id, space_pov));

    // Build topic: bytes4(objectType) | bytes16(objectId)
    let mut topic = vec![0u8; 32];
    topic[0..4].copy_from_slice(&[0x00, 0x00, 0x00, 0x01]); // object type
    topic[4..20].copy_from_slice(&[0xDD; 16]); // object id

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![],
        action: actions::UPVOTED.to_vec(),
        topic,
        data: encoded,
    }
}

/// Generate a downvote action.
fn make_downvote_action(voter_id: [u8; 16]) -> Action {
    let version = 1u16;
    let group_id = FixedBytes::<16>::from([0xCCu8; 16]);
    let space_pov = FixedBytes::<16>::from([0xDDu8; 16]);
    let encoded = VoteDataType::abi_encode(&(version, group_id, space_pov));

    let mut topic = vec![0u8; 32];
    topic[0..4].copy_from_slice(&[0x00, 0x00, 0x00, 0x02]);
    topic[4..20].copy_from_slice(&[0xEE; 16]);

    Action {
        from_id: voter_id.to_vec(),
        to_id: vec![],
        action: actions::DOWNVOTED.to_vec(),
        topic,
        data: encoded,
    }
}

/// Generate an editor added action.
///
/// ZC16 format: topic is target space ID (zeros for self), data is ABI-encoded address.
fn make_editor_added_action(space_id: [u8; 16]) -> Action {
    // ZC16: Data contains ABI-encoded address (12 zeros + 20 byte address)
    let mut data = vec![0u8; 32];
    data[12..32].copy_from_slice(&[0xFF; 20]);

    Action {
        from_id: space_id.to_vec(),
        to_id: vec![],
        action: actions::EDITOR_ADDED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (zeros for self)
        data,
    }
}

/// Generate a member added action.
///
/// ZC16 format: topic is target space ID (zeros for self), data is ABI-encoded address.
fn make_member_added_action(space_id: [u8; 16]) -> Action {
    // ZC16: Data contains ABI-encoded address (12 zeros + 20 byte address)
    let mut data = vec![0u8; 32];
    data[12..32].copy_from_slice(&[0xEE; 20]);

    Action {
        from_id: space_id.to_vec(),
        to_id: vec![],
        action: actions::MEMBER_ADDED.to_vec(),
        topic: vec![0u8; 32], // ZC16: topic is target space ID (zeros for self)
        data,
    }
}

/// Generate a space left action.
fn make_space_left_action(member_id: [u8; 16], space_id: [u8; 16]) -> Action {
    Action {
        from_id: member_id.to_vec(),
        to_id: space_id.to_vec(),
        action: actions::SPACE_LEFT.to_vec(),
        topic: vec![0; 32],
        data: vec![],
    }
}

/// Generate a subspace verified action.
fn make_subspace_verified_action(parent_id: [u8; 16], child_id: [u8; 16]) -> Action {
    Action {
        from_id: parent_id.to_vec(),
        to_id: child_id.to_vec(),
        action: actions::SUBSPACE_VERIFIED.to_vec(),
        topic: vec![0; 32],
        data: vec![],
    }
}

/// Generate a space registered action.
fn make_space_registered_action(space_id: [u8; 16]) -> Action {
    Action {
        from_id: space_id.to_vec(),
        to_id: vec![],
        action: actions::SPACE_REGISTERED.to_vec(),
        topic: vec![0xAA; 32], // network_id
        data: vec![],
    }
}

// =============================================================================
// Governance Pipeline Benchmarks
// =============================================================================

fn bench_governance_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("governance_transform");
    let meta = test_meta();

    // Empty actions
    let empty_actions: Vec<Action> = vec![];
    group.bench_function("empty", |b| {
        b.iter(|| pipelines::governance::transform(black_box(&empty_actions), black_box(&meta)))
    });

    // Single proposal created with 1 action
    let single_proposal = vec![make_proposal_created_action([0x01; 16], 1)];
    group.bench_function("single_proposal_1_action", |b| {
        b.iter(|| pipelines::governance::transform(black_box(&single_proposal), black_box(&meta)))
    });

    // Single proposal created with 5 actions
    let proposal_5_actions = vec![make_proposal_created_action([0x01; 16], 5)];
    group.bench_function("single_proposal_5_actions", |b| {
        b.iter(|| {
            pipelines::governance::transform(black_box(&proposal_5_actions), black_box(&meta))
        })
    });

    // Multiple proposal votes
    let votes: Vec<Action> = (0..10)
        .map(|i| make_proposal_voted_action([i as u8; 16], [0x01; 16]))
        .collect();
    group.bench_function("ten_votes", |b| {
        b.iter(|| pipelines::governance::transform(black_box(&votes), black_box(&meta)))
    });

    // Mixed governance actions (typical block)
    let mixed_actions = vec![
        make_proposal_created_action([0x01; 16], 3),
        make_proposal_voted_action([0x02; 16], [0x01; 16]),
        make_proposal_voted_action([0x03; 16], [0x01; 16]),
        make_proposal_voted_action([0x04; 16], [0x01; 16]),
        make_proposal_executed_action([0x01; 16]),
    ];
    group.bench_function("mixed_governance_block", |b| {
        b.iter(|| pipelines::governance::transform(black_box(&mixed_actions), black_box(&meta)))
    });

    // Large block with many proposals
    let large_block: Vec<Action> = (0..20)
        .flat_map(|i| {
            vec![
                make_proposal_created_action([i as u8; 16], 2),
                make_proposal_voted_action([(i + 100) as u8; 16], [i as u8; 16]),
            ]
        })
        .collect();
    group.bench_function("large_block_40_actions", |b| {
        b.iter(|| pipelines::governance::transform(black_box(&large_block), black_box(&meta)))
    });

    group.finish();
}

// =============================================================================
// Voting Pipeline Benchmarks
// =============================================================================

fn bench_voting_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("voting_transform");
    let meta = test_meta();

    // Empty actions
    let empty_actions: Vec<Action> = vec![];
    group.bench_function("empty", |b| {
        b.iter(|| pipelines::voting::transform(black_box(&empty_actions), black_box(&meta)))
    });

    // Single upvote
    let single_upvote = vec![make_upvote_action([0x01; 16])];
    group.bench_function("single_upvote", |b| {
        b.iter(|| pipelines::voting::transform(black_box(&single_upvote), black_box(&meta)))
    });

    // Multiple upvotes
    let upvotes: Vec<Action> = (0..10).map(|i| make_upvote_action([i as u8; 16])).collect();
    group.bench_function("ten_upvotes", |b| {
        b.iter(|| pipelines::voting::transform(black_box(&upvotes), black_box(&meta)))
    });

    // Mixed votes (upvotes, downvotes)
    let mixed_votes: Vec<Action> = (0..20)
        .map(|i| {
            if i % 2 == 0 {
                make_upvote_action([i as u8; 16])
            } else {
                make_downvote_action([i as u8; 16])
            }
        })
        .collect();
    group.bench_function("twenty_mixed_votes", |b| {
        b.iter(|| pipelines::voting::transform(black_box(&mixed_votes), black_box(&meta)))
    });

    // Large voting batch (stress test)
    let large_batch: Vec<Action> = (0..100)
        .map(|i| make_upvote_action([i as u8; 16]))
        .collect();
    group.bench_function("hundred_votes", |b| {
        b.iter(|| pipelines::voting::transform(black_box(&large_batch), black_box(&meta)))
    });

    group.finish();
}

// =============================================================================
// Membership Pipeline Benchmarks
// =============================================================================

fn bench_membership_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("membership_transform");
    let meta = test_meta();

    // Empty actions
    let empty_actions: Vec<Action> = vec![];
    group.bench_function("empty", |b| {
        b.iter(|| pipelines::membership::transform(black_box(&empty_actions), black_box(&meta)))
    });

    // Single editor added
    let single_editor = vec![make_editor_added_action([0x01; 16])];
    group.bench_function("single_editor_added", |b| {
        b.iter(|| pipelines::membership::transform(black_box(&single_editor), black_box(&meta)))
    });

    // Multiple membership changes
    let membership_changes: Vec<Action> = (0..10)
        .flat_map(|i| {
            vec![
                make_editor_added_action([i as u8; 16]),
                make_member_added_action([i as u8; 16]),
            ]
        })
        .collect();
    group.bench_function("twenty_membership_changes", |b| {
        b.iter(|| {
            pipelines::membership::transform(black_box(&membership_changes), black_box(&meta))
        })
    });

    // Mixed with space left
    let mixed_membership = vec![
        make_editor_added_action([0x01; 16]),
        make_member_added_action([0x01; 16]),
        make_space_left_action([0x02; 16], [0x01; 16]),
        make_editor_added_action([0x03; 16]),
    ];
    group.bench_function("mixed_membership", |b| {
        b.iter(|| pipelines::membership::transform(black_box(&mixed_membership), black_box(&meta)))
    });

    group.finish();
}

// =============================================================================
// Trust Pipeline Benchmarks
// =============================================================================

fn bench_trust_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("trust_transform");
    let meta = test_meta();

    // Empty actions
    let empty_actions: Vec<Action> = vec![];
    group.bench_function("empty", |b| {
        b.iter(|| pipelines::trust::transform(black_box(&empty_actions), black_box(&meta)))
    });

    // Single subspace verified
    let single_verified = vec![make_subspace_verified_action([0x01; 16], [0x02; 16])];
    group.bench_function("single_verified", |b| {
        b.iter(|| pipelines::trust::transform(black_box(&single_verified), black_box(&meta)))
    });

    // Multiple trust extensions
    let trust_extensions: Vec<Action> = (0..10)
        .map(|i| make_subspace_verified_action([i as u8; 16], [(i + 1) as u8; 16]))
        .collect();
    group.bench_function("ten_trust_extensions", |b| {
        b.iter(|| pipelines::trust::transform(black_box(&trust_extensions), black_box(&meta)))
    });

    group.finish();
}

// =============================================================================
// Spaces Pipeline Benchmarks
// =============================================================================

fn bench_spaces_transform(c: &mut Criterion) {
    let mut group = c.benchmark_group("spaces_transform");
    let meta = test_meta();

    // Empty actions
    let empty_actions: Vec<Action> = vec![];
    group.bench_function("empty", |b| {
        b.iter(|| pipelines::spaces::transform(black_box(&empty_actions), black_box(&meta)))
    });

    // Single space registered
    let single_space = vec![make_space_registered_action([0x01; 16])];
    group.bench_function("single_space", |b| {
        b.iter(|| pipelines::spaces::transform(black_box(&single_space), black_box(&meta)))
    });

    // Multiple space registrations
    let spaces: Vec<Action> = (0..10)
        .map(|i| make_space_registered_action([i as u8; 16]))
        .collect();
    group.bench_function("ten_spaces", |b| {
        b.iter(|| pipelines::spaces::transform(black_box(&spaces), black_box(&meta)))
    });

    group.finish();
}

// =============================================================================
// Combined Pipeline Benchmark (All Pipelines)
// =============================================================================

fn bench_all_pipelines(c: &mut Criterion) {
    let mut group = c.benchmark_group("all_pipelines");
    let meta = test_meta();

    // Simulate a realistic block with mixed action types
    let mixed_block = vec![
        // Governance
        make_proposal_created_action([0x01; 16], 2),
        make_proposal_voted_action([0x02; 16], [0x01; 16]),
        make_proposal_voted_action([0x03; 16], [0x01; 16]),
        make_proposal_executed_action([0x01; 16]),
        // Voting
        make_upvote_action([0x04; 16]),
        make_upvote_action([0x05; 16]),
        make_downvote_action([0x06; 16]),
        // Membership
        make_editor_added_action([0x07; 16]),
        make_member_added_action([0x07; 16]),
        // Trust
        make_subspace_verified_action([0x08; 16], [0x09; 16]),
        // Spaces
        make_space_registered_action([0x0A; 16]),
    ];

    group.bench_function("realistic_mixed_block", |b| {
        b.iter(|| {
            let _ = pipelines::governance::transform(black_box(&mixed_block), black_box(&meta));
            let _ = pipelines::voting::transform(black_box(&mixed_block), black_box(&meta));
            let _ = pipelines::membership::transform(black_box(&mixed_block), black_box(&meta));
            let _ = pipelines::trust::transform(black_box(&mixed_block), black_box(&meta));
            let _ = pipelines::spaces::transform(black_box(&mixed_block), black_box(&meta));
        })
    });

    // Large mixed block stress test
    let large_mixed_block: Vec<Action> = (0..50)
        .flat_map(|i| {
            vec![
                make_proposal_voted_action([i as u8; 16], [0x01; 16]),
                make_upvote_action([(i + 50) as u8; 16]),
                make_editor_added_action([(i + 100) as u8; 16]),
            ]
        })
        .collect();

    group.bench_function("large_mixed_block_150_actions", |b| {
        b.iter(|| {
            let _ =
                pipelines::governance::transform(black_box(&large_mixed_block), black_box(&meta));
            let _ = pipelines::voting::transform(black_box(&large_mixed_block), black_box(&meta));
            let _ =
                pipelines::membership::transform(black_box(&large_mixed_block), black_box(&meta));
            let _ = pipelines::trust::transform(black_box(&large_mixed_block), black_box(&meta));
            let _ = pipelines::spaces::transform(black_box(&large_mixed_block), black_box(&meta));
        })
    });

    group.finish();
}

// =============================================================================
// Action Filtering Benchmark
// =============================================================================

fn bench_action_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("action_filtering");

    // Test action matching performance
    let action_types = [
        actions::PROPOSAL_CREATED.to_vec(),
        actions::PROPOSAL_VOTED.to_vec(),
        actions::PROPOSAL_EXECUTED.to_vec(),
        actions::UPVOTED.to_vec(),
        actions::DOWNVOTED.to_vec(),
        actions::EDITOR_ADDED.to_vec(),
        actions::MEMBER_ADDED.to_vec(),
        actions::SPACE_REGISTERED.to_vec(),
        actions::SUBSPACE_VERIFIED.to_vec(),
    ];

    group.bench_function("match_single_action", |b| {
        let test_action = &actions::PROPOSAL_CREATED;
        b.iter(|| {
            black_box(actions::matches(
                black_box(&action_types[0]),
                black_box(test_action),
            ))
        })
    });

    group.bench_function("match_all_action_types", |b| {
        b.iter(|| {
            for action_type in &action_types {
                black_box(actions::matches(
                    black_box(action_type),
                    black_box(&actions::PROPOSAL_CREATED),
                ));
                black_box(actions::matches(
                    black_box(action_type),
                    black_box(&actions::PROPOSAL_VOTED),
                ));
                black_box(actions::matches(
                    black_box(action_type),
                    black_box(&actions::UPVOTED),
                ));
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_governance_transform,
    bench_voting_transform,
    bench_membership_transform,
    bench_trust_transform,
    bench_spaces_transform,
    bench_all_pipelines,
    bench_action_filtering,
);

criterion_main!(benches);
