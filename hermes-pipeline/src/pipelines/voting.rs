//! Pipeline: UPVOTED, DOWNVOTED, UNVOTED → curation.votes
//!
//! Converts permissionless voting actions to typed Hermes events with decoded data.

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::voting::{HermesVoteCast, VoteDirection};

use hermes_codec as decode;

use super::BlockMetadata;

/// Result of transforming voting actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub votes: Vec<HermesVoteCast>,
    pub upvotes: u64,
    pub downvotes: u64,
    pub unvotes: u64,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.votes.len()
    }
}

/// Transform all voting actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::UPVOTED) {
            let event = debug_span!(
                "convert.voting.upvoted",
                voter_id = %hex::encode(&action.from_id),
                object_id = %hex::encode(&action.topic[4..20])
            )
            .in_scope(|| convert_vote(action, meta, VoteDirection::Up, sequence))?;
            result.votes.push(event);
            result.upvotes += 1;
        } else if actions::matches(action_type, &actions::DOWNVOTED) {
            let event = debug_span!(
                "convert.voting.downvoted",
                voter_id = %hex::encode(&action.from_id),
                object_id = %hex::encode(&action.topic[4..20])
            )
            .in_scope(|| convert_vote(action, meta, VoteDirection::Down, sequence))?;
            result.votes.push(event);
            result.downvotes += 1;
        } else if actions::matches(action_type, &actions::UNVOTED) {
            let event = debug_span!(
                "convert.voting.unvoted",
                voter_id = %hex::encode(&action.from_id),
                object_id = %hex::encode(&action.topic[4..20])
            )
            .in_scope(|| convert_vote(action, meta, VoteDirection::None, sequence))?;
            result.votes.push(event);
            result.unvotes += 1;
        }
    }

    Ok(result)
}

/// Convert an UPVOTED, DOWNVOTED, or UNVOTED action to HermesVoteCast proto.
///
/// The action structure:
/// - from_id: voter_id (16 bytes) - voter's space ID
/// - topic: bytes32(bytes4(objectType) << 224) | (bytes16(objectId) << 96)
///   - topic[0..4]: object type (4 bytes)
///   - topic[4..20]: object ID (16 bytes)
/// - data: abi.encode(uint16(version), bytes16(groupId), bytes16(spacePOV))
fn convert_vote(
    action: &Action,
    meta: &BlockMetadata,
    direction: VoteDirection,
    sequence: u32,
) -> Result<HermesVoteCast> {
    // Extract object type and ID from topic
    let object_type = if action.topic.len() >= 4 {
        action.topic[0..4].to_vec()
    } else {
        vec![0; 4]
    };

    let object_id = if action.topic.len() >= 20 {
        action.topic[4..20].to_vec()
    } else {
        vec![0; 16]
    };

    // Decode the data field
    let (version, group_id, space_pov) = match decode::decode_vote_data(&action.data) {
        Ok(decoded) => (decoded.version as u32, decoded.group_id, decoded.space_pov),
        Err(e) => {
            warn!(
                error = %e,
                voter_id = %hex::encode(&action.from_id),
                "Failed to decode vote data"
            );
            (0, vec![0; 16], vec![0; 16])
        }
    };

    Ok(HermesVoteCast {
        voter_id: action.from_id.clone(),
        object_type,
        object_id,
        direction: direction as i32,
        version,
        group_id,
        space_pov,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Helper to get vote direction string for logging
pub fn get_vote_direction(vote: &HermesVoteCast) -> &'static str {
    match VoteDirection::try_from(vote.direction) {
        Ok(VoteDirection::Up) => "UP",
        Ok(VoteDirection::Down) => "DOWN",
        Ok(VoteDirection::None) => "NONE",
        Err(_) => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    fn make_vote_topic(object_type: [u8; 4], object_id: [u8; 16]) -> Vec<u8> {
        let mut topic = vec![0u8; 32];
        topic[0..4].copy_from_slice(&object_type);
        topic[4..20].copy_from_slice(&object_id);
        topic
    }

    #[test]
    fn test_convert_upvote_empty_data() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let object_id = [2u8; 16];

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::UPVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(&action, &test_meta(), VoteDirection::Up, 0).unwrap();
        assert_eq!(result.voter_id, vec![1; 16]);
        assert_eq!(result.object_type, object_type.to_vec());
        assert_eq!(result.object_id, object_id.to_vec());
        assert_eq!(result.direction, VoteDirection::Up as i32);
        // Default values when decode fails
        assert_eq!(result.version, 0);
        assert_eq!(result.group_id.len(), 16);
        assert_eq!(result.space_pov.len(), 16);
    }

    #[test]
    fn test_convert_downvote_empty_data() {
        let object_type = [0x00, 0x00, 0x00, 0x02];
        let object_id = [5u8; 16];
        let action = Action {
            from_id: vec![4; 16],
            to_id: vec![],
            action: actions::DOWNVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(&action, &test_meta(), VoteDirection::Down, 0).unwrap();
        assert_eq!(result.direction, VoteDirection::Down as i32);
        assert_eq!(result.version, 0);
        assert_eq!(result.group_id.len(), 16);
        assert_eq!(result.space_pov.len(), 16);
    }

    #[test]
    fn test_convert_unvote() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let object_id = [6u8; 16];
        let action = Action {
            from_id: vec![7; 16],
            to_id: vec![],
            action: actions::UNVOTED.to_vec(),
            topic: make_vote_topic(object_type, object_id),
            data: vec![],
        };

        let result = convert_vote(&action, &test_meta(), VoteDirection::None, 0).unwrap();
        assert_eq!(result.direction, VoteDirection::None as i32);
    }

    #[test]
    fn test_transform_counts() {
        let object_type = [0x00, 0x00, 0x00, 0x01];
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::UPVOTED.to_vec(),
                topic: make_vote_topic(object_type, [1u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![2; 16],
                to_id: vec![],
                action: actions::UPVOTED.to_vec(),
                topic: make_vote_topic(object_type, [2u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::DOWNVOTED.to_vec(),
                topic: make_vote_topic(object_type, [3u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![4; 16],
                to_id: vec![],
                action: actions::UNVOTED.to_vec(),
                topic: make_vote_topic(object_type, [4u8; 16]),
                data: vec![],
            },
            // Should NOT be included
            Action {
                from_id: vec![5; 16],
                to_id: vec![],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![6; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.total(), 4);
        assert_eq!(result.upvotes, 2);
        assert_eq!(result.downvotes, 1);
        assert_eq!(result.unvotes, 1);
    }

    #[test]
    fn test_get_vote_direction() {
        let vote = HermesVoteCast {
            voter_id: vec![],
            object_type: vec![],
            object_id: vec![],
            direction: VoteDirection::Up as i32,
            version: 0,
            group_id: vec![],
            space_pov: vec![],
            meta: None,
        };
        assert_eq!(get_vote_direction(&vote), "UP");
    }
}
