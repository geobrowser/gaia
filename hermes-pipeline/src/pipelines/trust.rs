//! Pipeline: SUBSPACE_VERIFIED/RELATED/TOPIC_DECLARED/REMOVED → space.trust.extensions
//!
//! Converts trust extension and revocation actions to HermesSpaceTrustExtension events.
//!
//! Action types:
//! - SUBSPACE_VERIFIED: Verified trust extension (explicit canonical trust)
//! - SUBSPACE_RELATED: Related trust extension (explicit non-canonical trust)
//! - SUBSPACE_TOPIC_DECLARED: Topic-based trust extension
//! - SUBSPACE_REMOVED: Trust revocation

use anyhow::Result;
use hermes_instrumentation::debug_span;

use hermes_relay::{Action, actions};
use hermes_schema::pb::space::{
    HermesSpaceTrustExtension, RelatedExtension, SubtopicExtension, VerifiedExtension,
    hermes_space_trust_extension,
};

use super::{BlockMetadata, HasMeta};
use hermes_schema::pb::blockchain_metadata::BlockchainMetadata;

/// A trust event with its type (added or removed).
#[derive(Debug, Clone)]
pub struct TrustEvent {
    pub event: HermesSpaceTrustExtension,
    pub is_removal: bool,
}

impl HasMeta for TrustEvent {
    fn meta(&self) -> Option<&BlockchainMetadata> {
        self.event.meta.as_ref()
    }
    fn meta_mut(&mut self) -> Option<&mut BlockchainMetadata> {
        self.event.meta.as_mut()
    }
}

/// Result of transforming trust actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed trust extension events ready for emission.
    pub events: Vec<TrustEvent>,
    /// Count of verified extensions.
    pub verified: u64,
    /// Count of related extensions.
    pub related: u64,
    /// Count of topic declarations.
    pub topic_declared: u64,
    /// Count of removals.
    pub removed: u64,
}

impl TransformResult {
    /// Total number of trust events.
    pub fn total(&self) -> u64 {
        self.verified + self.related + self.topic_declared + self.removed
    }
}

/// Transform all trust-related actions in a block.
///
/// Handles the following action types:
/// - SUBSPACE_VERIFIED: Verified trust extension
/// - SUBSPACE_RELATED: Related trust extension
/// - SUBSPACE_TOPIC_DECLARED: Topic-based trust extension
/// - SUBSPACE_ADDED: Generic addition (treated as verified for backwards compatibility)
/// - SUBSPACE_REMOVED: Trust revocation
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::SUBSPACE_VERIFIED) {
            let event = debug_span!(
                "convert.trust.verified",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_verified(action, meta, sequence))?;
            result.events.push(TrustEvent {
                event,
                is_removal: false,
            });
            result.verified += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_RELATED) {
            let event = debug_span!(
                "convert.trust.related",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_related(action, meta, sequence))?;
            result.events.push(TrustEvent {
                event,
                is_removal: false,
            });
            result.related += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_TOPIC_DECLARED) {
            let event = debug_span!(
                "convert.trust.topic_declared",
                source = %hex::encode(&action.from_id),
                subspace = %hex::encode(&action.topic[0..16]),
                topic = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_topic_declared(action, meta, sequence))?;
            result.events.push(TrustEvent {
                event,
                is_removal: false,
            });
            result.topic_declared += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_REMOVED) {
            let event = debug_span!(
                "convert.trust.removed",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_removed(action, meta, sequence))?;
            result.events.push(TrustEvent {
                event,
                is_removal: true,
            });
            result.removed += 1;
        }
    }

    Ok(result)
}

/// Convert a SUBSPACE_VERIFIED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_VERIFIED:
/// - from_id: parent_space_id (16 bytes)
/// - topic: [padding (16 bytes)][subspace_id (16 bytes)]
fn convert_verified(action: &Action, meta: &BlockMetadata, sequence: u32) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    // Target space ID is in the last 16 bytes of the topic field
    let target_space_id = action.topic[16..32].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_RELATED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_RELATED:
/// - from_id: parent_space_id (16 bytes)
/// - topic: [padding (16 bytes)][subspace_id (16 bytes)]
fn convert_related(action: &Action, meta: &BlockMetadata, sequence: u32) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[16..32].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::Related(
        RelatedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_TOPIC_DECLARED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_TOPIC_DECLARED:
/// - from_id: parent_space_id (16 bytes)
/// - topic: [subspace_id (16 bytes)][topic_id (16 bytes)]
fn convert_topic_declared(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    // Topic ID is in the last 16 bytes of the topic field
    let target_topic_id = action.topic[16..32].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::Subtopic(
        SubtopicExtension { target_topic_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_REMOVED action to HermesSpaceTrustExtension proto.
///
/// Uses the same structure as SUBSPACE_ADDED but represents a trust revocation.
fn convert_removed(action: &Action, meta: &BlockMetadata, sequence: u32) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[16..32].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Get the extension type as a string for logging.
pub fn get_extension_type(ext: &HermesSpaceTrustExtension) -> &'static str {
    match &ext.extension {
        Some(hermes_space_trust_extension::Extension::Verified(_)) => "verified",
        Some(hermes_space_trust_extension::Extension::Related(_)) => "related",
        Some(hermes_space_trust_extension::Extension::Subtopic(_)) => "subtopic",
        None => "unknown",
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

    fn make_topic_with_target(target: &[u8]) -> Vec<u8> {
        let mut topic = vec![0u8; 16]; // padding
        topic.extend_from_slice(target);
        topic
    }

    #[test]
    fn test_convert_subspace_verified() {
        let target = vec![2u8; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_VERIFIED.to_vec(),
            topic: make_topic_with_target(&target),
            data: vec![],
        };

        let result = convert_verified(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::Verified(v)) => {
                assert_eq!(v.target_space_id, target);
            }
            _ => panic!("Expected Verified extension"),
        }
    }

    #[test]
    fn test_convert_subspace_related() {
        let target = vec![2u8; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_RELATED.to_vec(),
            topic: make_topic_with_target(&target),
            data: vec![],
        };

        let result = convert_related(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::Related(r)) => {
                assert_eq!(r.target_space_id, target);
            }
            _ => panic!("Expected Related extension"),
        }
    }

    #[test]
    fn test_convert_subspace_topic_declared() {
        let subspace = vec![2u8; 16];
        let topic_id = vec![3u8; 16];
        let mut topic = subspace.clone();
        topic.extend_from_slice(&topic_id);

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_TOPIC_DECLARED.to_vec(),
            topic,
            data: vec![],
        };

        let result = convert_topic_declared(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::Subtopic(s)) => {
                assert_eq!(s.target_topic_id, topic_id);
            }
            _ => panic!("Expected Subtopic extension"),
        }
    }

    #[test]
    fn test_convert_subspace_removed() {
        let target = vec![2u8; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_REMOVED.to_vec(),
            topic: make_topic_with_target(&target),
            data: vec![],
        };

        let result = convert_removed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
    }

    #[test]
    fn test_transform_counts() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::SUBSPACE_VERIFIED.to_vec(),
                topic: make_topic_with_target(&[2u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::SUBSPACE_RELATED.to_vec(),
                topic: make_topic_with_target(&[4u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![5; 16],
                to_id: vec![],
                action: actions::SUBSPACE_TOPIC_DECLARED.to_vec(),
                topic: {
                    let mut t = vec![6u8; 16];
                    t.extend_from_slice(&[7u8; 16]);
                    t
                },
                data: vec![],
            },
            Action {
                from_id: vec![8; 16],
                to_id: vec![],
                action: actions::SUBSPACE_REMOVED.to_vec(),
                topic: make_topic_with_target(&[9u8; 16]),
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 4);
        assert_eq!(result.verified, 1);
        assert_eq!(result.related, 1);
        assert_eq!(result.topic_declared, 1);
        assert_eq!(result.removed, 1);
        assert_eq!(result.total(), 4);
    }
}
