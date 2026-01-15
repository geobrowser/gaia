//! Benchmarks for ABI decoding functions.
//!
//! These benchmarks measure the performance of decoding ABI-encoded blockchain data
//! into typed Rust structures.

use alloy::primitives::{Address, Bytes as PrimBytes, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::SolType;
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use hermes_pipeline::decode::{
    ProposalActionType, decode_space_id_arg, decode_flag_data, decode_proposal_created,
    decode_proposal_voted, decode_topic_declared, decode_vote_data, selectors,
};

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

/// Generate encoded proposal created data with a specified number of actions.
fn generate_proposal_created_data(num_actions: usize) -> Vec<u8> {
    let actions: Vec<TestAction> = (0..num_actions)
        .map(|i| {
            // Alternate between different action types
            let selector = match i % 5 {
                0 => selectors::ADD_MEMBER,
                1 => selectors::REMOVE_MEMBER,
                2 => selectors::ADD_EDITOR,
                3 => selectors::PUBLISH,
                _ => selectors::FLAG,
            };

            let mut calldata = selector.to_vec();
            // Add bytes16 space ID argument (right-padded to 32 bytes)
            calldata.extend_from_slice(&[(i % 256) as u8; 16]);
            calldata.extend_from_slice(&[0u8; 16]);

            TestAction {
                to: Address::ZERO,
                value: U256::from(i as u64),
                data: calldata.into(),
            }
        })
        .collect();

    let voting_mode = 1u8; // Slow mode
    ProposalCreatedDataType::abi_encode(&(voting_mode, actions))
}

/// Generate encoded proposal voted data.
fn generate_proposal_voted_data() -> Vec<u8> {
    let proposal_id = U256::from(12345u64);
    let vote_option = 1u8; // Yes
    ProposalVotedDataType::abi_encode(&(proposal_id, vote_option))
}

/// Generate encoded vote data for permissionless voting.
fn generate_vote_data() -> Vec<u8> {
    let version = 1u16;
    let group_id = FixedBytes::<16>::from([0xAAu8; 16]);
    let space_pov = FixedBytes::<16>::from([0xBBu8; 16]);
    VoteDataType::abi_encode(&(version, group_id, space_pov))
}

/// Generate calldata with function selector and bytes16 space ID argument.
fn generate_space_id_calldata() -> Vec<u8> {
    let mut calldata = selectors::ADD_MEMBER.to_vec();
    calldata.extend_from_slice(&[0x11u8; 16]); // bytes16 space ID
    calldata.extend_from_slice(&[0u8; 16]); // right-padding
    calldata
}

/// Generate encoded flag data (IPFS URI).
fn generate_flag_data() -> Vec<u8> {
    let uri: PrimBytes = "ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".into();
    alloy::sol_types::sol_data::Bytes::abi_encode(&uri)
}

/// Generate encoded topic declared data.
fn generate_topic_declared_data() -> Vec<u8> {
    let topic_id = FixedBytes::<16>::from([0x42u8; 16]);
    alloy::sol_types::sol_data::FixedBytes::<16>::abi_encode(&topic_id)
}

// =============================================================================
// Function Selector Matching Benchmarks
// =============================================================================

fn bench_proposal_action_type_from_calldata(c: &mut Criterion) {
    let mut group = c.benchmark_group("proposal_action_type");

    // Benchmark with known selector
    let known_calldata = generate_address_calldata();
    group.bench_function("known_selector", |b| {
        b.iter(|| ProposalActionType::from_calldata(black_box(&known_calldata)))
    });

    // Benchmark with unknown selector
    let unknown_calldata = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00];
    group.bench_function("unknown_selector", |b| {
        b.iter(|| ProposalActionType::from_calldata(black_box(&unknown_calldata)))
    });

    // Benchmark with short calldata (edge case)
    let short_calldata = vec![0xCA, 0x6D];
    group.bench_function("short_calldata", |b| {
        b.iter(|| ProposalActionType::from_calldata(black_box(&short_calldata)))
    });

    // Benchmark all selectors
    let all_selectors = [
        selectors::ADD_MEMBER,
        selectors::REMOVE_MEMBER,
        selectors::ADD_EDITOR,
        selectors::REMOVE_EDITOR,
        selectors::PUBLISH,
        selectors::FLAG,
        selectors::UNFLAG,
        selectors::UNRESTRICT_SPACE,
        selectors::UPDATE_VOTING_SETTINGS,
        selectors::PING,
    ];
    group.bench_function("all_selectors_sequential", |b| {
        b.iter(|| {
            for selector in &all_selectors {
                black_box(ProposalActionType::from_calldata(selector));
            }
        })
    });

    group.finish();
}

// =============================================================================
// Space ID Decoding Benchmarks
// =============================================================================

fn bench_decode_space_id_arg(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_space_id_arg");

    let valid_calldata = generate_space_id_calldata();
    group.bench_function("valid_calldata", |b| {
        b.iter(|| decode_space_id_arg(black_box(&valid_calldata)))
    });

    // Too short calldata
    let short_calldata = selectors::ADD_MEMBER.to_vec();
    group.bench_function("too_short", |b| {
        b.iter(|| decode_space_id_arg(black_box(&short_calldata)))
    });

    group.finish();
}

// =============================================================================
// Proposal Created Decoding Benchmarks
// =============================================================================

fn bench_decode_proposal_created(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_proposal_created");

    // Empty data
    group.bench_function("empty_data", |b| {
        b.iter(|| decode_proposal_created(black_box(&[])))
    });

    // Single action
    let single_action_data = generate_proposal_created_data(1);
    group.bench_function("single_action", |b| {
        b.iter(|| decode_proposal_created(black_box(&single_action_data)))
    });

    // 5 actions (typical batch proposal)
    let five_actions_data = generate_proposal_created_data(5);
    group.bench_function("five_actions", |b| {
        b.iter(|| decode_proposal_created(black_box(&five_actions_data)))
    });

    // 10 actions (larger batch)
    let ten_actions_data = generate_proposal_created_data(10);
    group.bench_function("ten_actions", |b| {
        b.iter(|| decode_proposal_created(black_box(&ten_actions_data)))
    });

    // 50 actions (stress test)
    let fifty_actions_data = generate_proposal_created_data(50);
    group.bench_function("fifty_actions", |b| {
        b.iter(|| decode_proposal_created(black_box(&fifty_actions_data)))
    });

    group.finish();
}

// =============================================================================
// Proposal Voted Decoding Benchmarks
// =============================================================================

fn bench_decode_proposal_voted(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_proposal_voted");

    let valid_data = generate_proposal_voted_data();
    group.bench_function("valid_data", |b| {
        b.iter(|| decode_proposal_voted(black_box(&valid_data)))
    });

    // Empty data (error case)
    group.bench_function("empty_data", |b| {
        b.iter(|| decode_proposal_voted(black_box(&[])))
    });

    group.finish();
}

// =============================================================================
// Vote Data Decoding Benchmarks
// =============================================================================

fn bench_decode_vote_data(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_vote_data");

    let valid_data = generate_vote_data();
    group.bench_function("valid_data", |b| {
        b.iter(|| decode_vote_data(black_box(&valid_data)))
    });

    // Empty data (returns default)
    group.bench_function("empty_data", |b| {
        b.iter(|| decode_vote_data(black_box(&[])))
    });

    group.finish();
}

// =============================================================================
// Topic Declared Decoding Benchmarks
// =============================================================================

fn bench_decode_topic_declared(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_topic_declared");

    let valid_data = generate_topic_declared_data();
    group.bench_function("valid_data", |b| {
        b.iter(|| decode_topic_declared(black_box(&valid_data)))
    });

    // Empty data (returns default)
    group.bench_function("empty_data", |b| {
        b.iter(|| decode_topic_declared(black_box(&[])))
    });

    group.finish();
}

// =============================================================================
// Flag Data Decoding Benchmarks
// =============================================================================

fn bench_decode_flag_data(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_flag_data");

    let valid_data = generate_flag_data();
    group.bench_function("valid_data", |b| {
        b.iter(|| decode_flag_data(black_box(&valid_data)))
    });

    // Empty data (returns empty string)
    group.bench_function("empty_data", |b| {
        b.iter(|| decode_flag_data(black_box(&[])))
    });

    // Long URI
    let long_uri: PrimBytes = "ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG/very/long/path/to/some/content/that/is/deeply/nested/in/the/filesystem".into();
    let long_data = alloy::sol_types::sol_data::Bytes::abi_encode(&long_uri);
    group.bench_function("long_uri", |b| {
        b.iter(|| decode_flag_data(black_box(&long_data)))
    });

    group.finish();
}

// =============================================================================
// Combined Decoding Benchmark (Simulating Real-World Usage)
// =============================================================================

fn bench_combined_decoding(c: &mut Criterion) {
    let mut group = c.benchmark_group("combined_decoding");

    // Simulate processing a typical block with mixed actions
    let proposal_created_data = generate_proposal_created_data(3);
    let proposal_voted_data = generate_proposal_voted_data();
    let vote_data = generate_vote_data();
    let flag_data = generate_flag_data();

    group.bench_function("typical_block_mix", |b| {
        b.iter(|| {
            // Decode proposal created
            let _ = decode_proposal_created(black_box(&proposal_created_data));

            // Decode 5 proposal votes
            for _ in 0..5 {
                let _ = decode_proposal_voted(black_box(&proposal_voted_data));
            }

            // Decode 10 permissionless votes
            for _ in 0..10 {
                let _ = decode_vote_data(black_box(&vote_data));
            }

            // Decode 2 flag actions
            for _ in 0..2 {
                let _ = decode_flag_data(black_box(&flag_data));
            }
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_proposal_action_type_from_calldata,
    bench_decode_space_id_arg,
    bench_decode_proposal_created,
    bench_decode_proposal_voted,
    bench_decode_vote_data,
    bench_decode_topic_declared,
    bench_decode_flag_data,
    bench_combined_decoding,
);

criterion_main!(benches);
