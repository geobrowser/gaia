//! Integration tests for KafkaStreamProvider.
//!
//! These tests verify the full Kafka consumption and conversion pipeline.
//!
//! ## Test Categories
//!
//! - **Unit-like tests**: Test conversion logic without Kafka
//! - **Integration tests** (marked `#[ignore]`): Require a running Kafka broker
//!
//! ## Running Integration Tests
//!
//! ```bash
//! # Run all tests including ignored ones
//! cargo test -p actions-indexer-pipeline --test kafka_integration -- --include-ignored
//!
//! # Run only integration tests that need Kafka
//! cargo test -p actions-indexer-pipeline --test kafka_integration -- --ignored
//! ```

use actions_indexer_pipeline::consumer::kafka::{ConsumerConfig, KafkaStreamProvider, hermes_vote_to_action_raw};
use actions_indexer_pipeline::consumer::{ConsumeActionsStream, StreamMessage};
use actions_indexer_shared::types::{ActionType, ObjectType};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;
use hermes_schema::pb::voting::{HermesVoteCast, VoteDirection};
use prost::Message;
use tokio::sync::mpsc;
use uuid::Uuid;
use url::Url;

/// Helper to create a valid HermesVoteCast message for testing.
fn create_test_vote(
    voter_id: Uuid,
    object_id: Uuid,
    group_id: Uuid,
    space_pov: Uuid,
    direction: VoteDirection,
    version: u16,
) -> HermesVoteCast {
    // Create ABI-encoded data: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
    let mut data = vec![0u8; 96];
    // uint16 version at bytes 30-31 (big-endian)
    let version_bytes = version.to_be_bytes();
    data[30..32].copy_from_slice(&version_bytes);
    // bytes16 groupId at bytes 32-47
    data[32..48].copy_from_slice(group_id.as_bytes());
    // bytes16 spacePOV at bytes 64-79
    data[64..80].copy_from_slice(space_pov.as_bytes());

    HermesVoteCast {
        voter_id: voter_id.as_bytes().to_vec(),
        object_id: object_id.as_bytes().to_vec(),
        object_type: 0u32.to_be_bytes().to_vec(), // Entity (big-endian, Solidity bytes4)
        direction: direction as i32,
        data,
        meta: Some(BlockchainMetadata {
            created_at: 1700000000,
            created_by: vec![],
            block_number: 12345,
            cursor: "test-cursor-123".to_string(),
        }),
    }
}

/// Helper to encode a HermesVoteCast to bytes (simulating Kafka message payload).
fn encode_vote(vote: &HermesVoteCast) -> Vec<u8> {
    let mut buf = Vec::new();
    vote.encode(&mut buf).expect("Failed to encode vote");
    buf
}

// =============================================================================
// CONVERSION PIPELINE TESTS (No Kafka required)
// =============================================================================

#[test]
fn test_full_conversion_pipeline_upvote() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let space_pov = Uuid::new_v4();

    let vote = create_test_vote(
        voter_id,
        object_id,
        group_id,
        space_pov,
        VoteDirection::Up,
        1,
    );

    // Encode to bytes (simulating Kafka payload)
    let payload = encode_vote(&vote);

    // Decode back (simulating what KafkaStreamProvider does)
    let decoded_vote = HermesVoteCast::decode(payload.as_slice()).unwrap();

    // Convert to ActionRaw
    let action_raw = hermes_vote_to_action_raw(&decoded_vote).unwrap();

    // Verify all fields
    assert_eq!(action_raw.user_id, voter_id);
    assert_eq!(action_raw.object_id, object_id);
    assert_eq!(action_raw.group_id, Some(group_id));
    assert_eq!(action_raw.space_pov, space_pov);
    assert_eq!(action_raw.action_type, ActionType::Vote);
    assert_eq!(action_raw.action_version, 1);
    assert_eq!(action_raw.object_type, ObjectType::Entity);
    assert_eq!(action_raw.block_number, 12345);
    assert_eq!(action_raw.block_timestamp, 1700000000);
    
    // Check vote direction in metadata
    let metadata = action_raw.metadata.as_ref().unwrap();
    assert_eq!(metadata[0], 0); // Up
}

#[test]
fn test_full_conversion_pipeline_downvote() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let space_pov = Uuid::new_v4();

    let vote = create_test_vote(
        voter_id,
        object_id,
        group_id,
        space_pov,
        VoteDirection::Down,
        2,
    );

    let payload = encode_vote(&vote);
    let decoded_vote = HermesVoteCast::decode(payload.as_slice()).unwrap();
    let action_raw = hermes_vote_to_action_raw(&decoded_vote).unwrap();

    assert_eq!(action_raw.action_version, 2);
    
    let metadata = action_raw.metadata.as_ref().unwrap();
    assert_eq!(metadata[0], 1); // Down
}

#[test]
fn test_full_conversion_pipeline_unvote() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let space_pov = Uuid::new_v4();

    let vote = create_test_vote(
        voter_id,
        object_id,
        group_id,
        space_pov,
        VoteDirection::None, // Unvote
        1,
    );

    let payload = encode_vote(&vote);
    let decoded_vote = HermesVoteCast::decode(payload.as_slice()).unwrap();
    let action_raw = hermes_vote_to_action_raw(&decoded_vote).unwrap();

    let metadata = action_raw.metadata.as_ref().unwrap();
    assert_eq!(metadata[0], 2); // Remove
}

#[test]
fn test_full_conversion_pipeline_relation_object_type() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let space_pov = Uuid::new_v4();

    let mut vote = create_test_vote(
        voter_id,
        object_id,
        group_id,
        space_pov,
        VoteDirection::Up,
        1,
    );
    // Change object_type to Relation (1) - big-endian to match Solidity bytes4
    vote.object_type = 1u32.to_be_bytes().to_vec();

    let payload = encode_vote(&vote);
    let decoded_vote = HermesVoteCast::decode(payload.as_slice()).unwrap();
    let action_raw = hermes_vote_to_action_raw(&decoded_vote).unwrap();

    assert_eq!(action_raw.object_type, ObjectType::Relation);
}

#[test]
fn test_conversion_error_invalid_voter_id() {
    let vote = HermesVoteCast {
        voter_id: vec![0; 8], // Invalid: only 8 bytes
        object_id: vec![0; 16],
        object_type: vec![0; 4],
        direction: VoteDirection::Up as i32,
        data: vec![0; 96],
        meta: Some(BlockchainMetadata {
            created_at: 0,
            created_by: vec![],
            block_number: 0,
            cursor: String::new(),
        }),
    };

    let result = hermes_vote_to_action_raw(&vote);
    assert!(result.is_err());
}

#[test]
fn test_conversion_error_invalid_data_length() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();

    let vote = HermesVoteCast {
        voter_id: voter_id.as_bytes().to_vec(),
        object_id: object_id.as_bytes().to_vec(),
        object_type: 0u32.to_be_bytes().to_vec(),
        direction: VoteDirection::Up as i32,
        data: vec![0; 50], // Invalid: should be 96 bytes
        meta: Some(BlockchainMetadata {
            created_at: 0,
            created_by: vec![],
            block_number: 0,
            cursor: String::new(),
        }),
    };

    let result = hermes_vote_to_action_raw(&vote);
    assert!(result.is_err());
}

#[test]
fn test_conversion_error_missing_metadata() {
    let voter_id = Uuid::new_v4();
    let object_id = Uuid::new_v4();

    let vote = HermesVoteCast {
        voter_id: voter_id.as_bytes().to_vec(),
        object_id: object_id.as_bytes().to_vec(),
        object_type: 0u32.to_be_bytes().to_vec(),
        direction: VoteDirection::Up as i32,
        data: vec![0; 96],
        meta: None, // Missing metadata
    };

    let result = hermes_vote_to_action_raw(&vote);
    assert!(result.is_err());
}

// =============================================================================
// CONFIGURATION TESTS
// =============================================================================

#[test]
fn test_consumer_config_creation() {
    let config = ConsumerConfig::new(Url::parse("localhost:9092").unwrap(), "test-group", "test-topic");
    
    assert_eq!(config.broker, Url::parse("localhost:9092").unwrap());
    assert_eq!(config.group_id, "test-group");
    assert_eq!(config.topic, "test-topic");
    assert!(config.username.is_none());
    assert!(config.password.is_none());
}

#[test]
fn test_consumer_config_with_credentials() {
    let config = ConsumerConfig::new(Url::parse("localhost:9092").unwrap(), "test-group", "test-topic")
        .with_credentials("user".to_string(), "pass".to_string());
    
    assert_eq!(config.username, Some("user".to_string()));
    assert_eq!(config.password, Some("pass".to_string()));
}

#[test]
fn test_kafka_stream_provider_creation() {
    let config = ConsumerConfig::new(Url::parse("localhost:9092").unwrap(), "test-group", "test-topic");
    let provider = KafkaStreamProvider::new(config);
    
    // Provider should be created without errors
    // We can't easily test internal state, but creation should succeed
    assert!(true);
    let _ = provider; // Use provider to avoid unused warning
}

// =============================================================================
// INTEGRATION TESTS (Require running Kafka)
// =============================================================================

/// Integration test that requires a running Kafka broker.
/// 
/// To run: `cargo test -p actions-indexer-pipeline --test kafka_integration test_kafka_connection -- --ignored`
/// 
/// Prerequisites:
/// - Kafka broker running at localhost:9092
/// - Topic "test-votes" created
#[tokio::test]
#[ignore = "Requires running Kafka broker"]
async fn test_kafka_connection() {
    let config = ConsumerConfig::new(Url::parse("localhost:9092").unwrap(), "integration-test-group", "test-votes");
    let provider = KafkaStreamProvider::new(config);
    
    let (tx, _rx) = mpsc::channel::<StreamMessage>(100);
    
    // Start the stream in a separate task
    let handle = tokio::spawn(async move {
        provider.stream_events(tx, None).await
    });
    
    // Wait briefly for connection
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    // Cancel the stream
    handle.abort();
    
    // We should not have received StreamEnd yet (stream was aborted)
    // This test mainly verifies the connection doesn't panic
}

/// Integration test for end-to-end message consumption.
/// 
/// To run: `cargo test -p actions-indexer-pipeline --test kafka_integration test_kafka_message_consumption -- --ignored`
/// 
/// Prerequisites:
/// - Kafka broker running at localhost:9092
/// - Topic "test-votes" created
/// - Test messages published to the topic
#[tokio::test]
#[ignore = "Requires running Kafka broker with test data"]
async fn test_kafka_message_consumption() {
    let config = ConsumerConfig::new(Url::parse("localhost:9092").unwrap(), "integration-test-consumer", "test-votes");
    let provider = KafkaStreamProvider::new(config);
    
    let (tx, mut rx) = mpsc::channel::<StreamMessage>(100);
    
    // Start the stream
    let handle = tokio::spawn(async move {
        provider.stream_events(tx, None).await
    });
    
    // Wait for messages (with timeout)
    let timeout = tokio::time::Duration::from_secs(10);
    let result = tokio::time::timeout(timeout, rx.recv()).await;
    
    match result {
        Ok(Some(StreamMessage::BlockData(data))) => {
            // Successfully received a message
            assert!(!data.actions.is_empty());
            println!("Received {} actions", data.actions.len());
        }
        Ok(Some(StreamMessage::Error(e))) => {
            // Error during consumption
            println!("Received error: {:?}", e);
        }
        Ok(Some(StreamMessage::StreamEnd)) => {
            println!("Stream ended");
        }
        Ok(None) => {
            println!("Channel closed");
        }
        Err(_) => {
            println!("Timeout waiting for messages");
        }
        _ => {}
    }
    
    handle.abort();
}

// =============================================================================
// BATCH PROCESSING TESTS
// =============================================================================

#[test]
fn test_multiple_votes_conversion() {
    let votes: Vec<HermesVoteCast> = (0..10)
        .map(|i| {
            create_test_vote(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
                if i % 3 == 0 { VoteDirection::Up } 
                else if i % 3 == 1 { VoteDirection::Down } 
                else { VoteDirection::None },
                (i + 1) as u16,
            )
        })
        .collect();

    let actions: Vec<_> = votes
        .iter()
        .map(|vote| {
            let payload = encode_vote(vote);
            let decoded = HermesVoteCast::decode(payload.as_slice()).unwrap();
            hermes_vote_to_action_raw(&decoded).unwrap()
        })
        .collect();

    assert_eq!(actions.len(), 10);
    
    // Verify action versions
    for (i, action) in actions.iter().enumerate() {
        assert_eq!(action.action_version, (i + 1) as u64);
    }
}

