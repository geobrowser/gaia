//! Pipeline: TOPIC_DECLARED → space.topics
//!
//! Converts topic declaration actions to typed Hermes events with decoded data.

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::topics::HermesTopicDeclared;

use hermes_codec as decode;

use super::BlockMetadata;

/// Result of transforming topic actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub topics_declared: Vec<HermesTopicDeclared>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.topics_declared.len()
    }
}

/// Transform all topic declaration actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::TOPIC_DECLARED) {
            let event = debug_span!(
                "convert.topics.declared",
                space_id = %hex::encode(&action.from_id),
                topic_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_topic_declared(action, meta, sequence))?;
            result.topics_declared.push(event);
        }
    }

    Ok(result)
}

/// Convert a TOPIC_DECLARED action to HermesTopicDeclared proto.
///
/// The action structure:
/// - from_id: space_id (16 bytes) - space declaring topic
/// - data: abi.encode(bytes16(topicId))
fn convert_topic_declared(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesTopicDeclared> {
    // Decode the topic_id from the data field
    let topic_id = match decode::decode_topic_declared(&action.data) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                space_id = %hex::encode(&action.from_id),
                "Failed to decode topic declared data"
            );
            vec![0; 16]
        }
    };

    Ok(HermesTopicDeclared {
        space_id: action.from_id.clone(),
        topic_id,
        meta: Some(meta.to_proto(sequence)),
    })
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

    #[test]
    fn test_convert_topic_declared_empty_data() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::TOPIC_DECLARED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_topic_declared(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        // Empty data defaults to zero-filled 16-byte UUID
        assert_eq!(result.topic_id, vec![0; 16]);
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_DECLARED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::TOPIC_DECLARED.to_vec(),
                topic: vec![4; 32],
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
        assert_eq!(result.topics_declared.len(), 2);
        assert_eq!(result.total(), 2);
    }
}
