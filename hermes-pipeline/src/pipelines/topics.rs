//! Pipeline: TOPIC_SET / TOPIC_UNSET → space.topics
//!
//! Converts topic set and unset actions to typed `HermesTopicDeclared` /
//! `HermesTopicRemoved` events with decoded data.
//! (Wire format keeps the legacy `HermesTopicDeclared` / `HermesTopicRemoved` names;
//! the contract selectors renamed from `TOPIC_DECLARED` / `TOPIC_REMOVED` to
//! `TOPIC_SET` / `TOPIC_UNSET` in Governance V2.)

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_relay::{Action, actions};
use hermes_schema::pb::topics::{HermesTopicDeclared, HermesTopicRemoved};

use crate::decode;

use super::BlockMetadata;

/// Result of transforming topic actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub topics_declared: Vec<HermesTopicDeclared>,
    pub topics_removed: Vec<HermesTopicRemoved>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.topics_declared.len() + self.topics_removed.len()
    }
}

/// Transform all topic-related actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::TOPIC_SET) {
            let event = debug_span!(
                "convert.topics.declared",
                space_id = %hex::encode(&action.from_id),
                topic_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_topic_declared(action, meta, sequence))?;
            result.topics_declared.push(event);
        } else if actions::matches(action_type, &actions::TOPIC_UNSET) {
            let event = debug_span!(
                "convert.topics.removed",
                space_id = %hex::encode(&action.from_id),
                topic_id = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_topic_removed(action, meta, sequence))?;
            result.topics_removed.push(event);
        }
    }

    Ok(result)
}

/// Convert a TOPIC_SET action to HermesTopicDeclared proto.
///
/// The action structure:
/// - from_id: space_id (16 bytes) - space declaring topic
/// - topic: bytes32(bytes16(topicId) | padding)
/// - data: optional topic metadata payload, ignored for id decoding
fn convert_topic_declared(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesTopicDeclared> {
    let topic_id = match decode::decode_topic_declared(&action.topic) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                space_id = %hex::encode(&action.from_id),
                "Failed to decode topic declared topic field"
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

/// Convert a TOPIC_UNSET action to HermesTopicRemoved proto.
///
/// The action structure mirrors TOPIC_SET:
/// - from_id: space_id (16 bytes) - space removing topic
/// - topic: bytes32(bytes16(topicId) | padding)
/// - data: empty
fn convert_topic_removed(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesTopicRemoved> {
    let topic_id = match decode::decode_topic_declared(&action.topic) {
        Ok(id) => id,
        Err(e) => {
            warn!(
                error = %e,
                space_id = %hex::encode(&action.from_id),
                "Failed to decode topic removed topic field"
            );
            vec![0; 16]
        }
    };

    Ok(HermesTopicRemoved {
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
    fn test_convert_topic_declared_uses_topic_field() {
        let mut topic = vec![2; 16];
        topic.extend_from_slice(&[0; 16]);

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::TOPIC_SET.to_vec(),
            topic,
            data: vec![9; 32],
        };

        let result = convert_topic_declared(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.topic_id, vec![2; 16]);
    }

    #[test]
    fn test_convert_topic_declared_empty_topic() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::TOPIC_SET.to_vec(),
            topic: vec![],
            data: vec![9; 32],
        };

        let result = convert_topic_declared(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.topic_id, vec![0; 16]);
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_SET.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::TOPIC_SET.to_vec(),
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

    #[test]
    fn test_convert_topic_removed_uses_topic_field() {
        let mut topic = vec![2; 16];
        topic.extend_from_slice(&[0; 16]);

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::TOPIC_UNSET.to_vec(),
            topic,
            data: vec![],
        };

        let result = convert_topic_removed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.topic_id, vec![2; 16]);
    }

    #[test]
    fn test_convert_topic_removed_empty_topic() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::TOPIC_UNSET.to_vec(),
            topic: vec![],
            data: vec![],
        };

        let result = convert_topic_removed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.topic_id, vec![0; 16]);
    }

    #[test]
    fn test_transform_counts_remove() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_UNSET.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::TOPIC_UNSET.to_vec(),
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
        assert_eq!(result.topics_declared.len(), 0);
        assert_eq!(result.topics_removed.len(), 2);
        assert_eq!(result.total(), 2);
    }

    /// Declare + remove for the same space within a single block.
    /// Both must end up in the result with their original block-sequence
    /// indexes preserved on the proto metadata, so the downstream Kafka
    /// emit order matches the on-chain order.
    #[test]
    fn test_transform_interleaved_declare_remove() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_SET.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_UNSET.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::TOPIC_SET.to_vec(),
                topic: vec![3; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.topics_declared.len(), 2);
        assert_eq!(result.topics_removed.len(), 1);
        assert_eq!(result.total(), 3);

        // Sequence indexes from BlockMetadata::to_proto reflect the per-block
        // action index, so the consumer can interleave events correctly.
        let declared_seqs: Vec<u32> = result
            .topics_declared
            .iter()
            .map(|e| e.meta.as_ref().unwrap().sequence)
            .collect();
        assert_eq!(declared_seqs, vec![0, 2]);

        let removed_seqs: Vec<u32> = result
            .topics_removed
            .iter()
            .map(|e| e.meta.as_ref().unwrap().sequence)
            .collect();
        assert_eq!(removed_seqs, vec![1]);
    }
}
