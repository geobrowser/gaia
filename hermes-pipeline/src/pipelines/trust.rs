//! Pipeline: SUBSPACE_VERIFIED/RELATED/TOPIC_SET → space.trust.extensions
//!
//! Converts trust extension and revocation actions to HermesSpaceTrustExtension events.
//!
//! Action types:
//! - SUBSPACE_VERIFIED: Verified trust extension (explicit canonical trust)
//! - SUBSPACE_RELATED: Related trust extension (explicit non-canonical trust)
//! - SUBSPACE_TOPIC_SET: Topic-based trust extension
//! - SUBSPACE_UNVERIFIED: Verified trust removal
//! - SUBSPACE_UNRELATED: Related trust removal
//! - SUBSPACE_TOPIC_UNSET: Topic trust removal

use anyhow::Result;
use hermes_instrumentation::debug_span;

use hermes_relay::{Action, actions};
use hermes_schema::pb::space::{
    HermesSpaceTrustExtension, RelatedExtension, RelatedRemoval, SubtopicExtension,
    SubtopicRemoval, VerifiedExtension, VerifiedRemoval, hermes_space_trust_extension,
};

use super::BlockMetadata;

/// Result of transforming trust actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed trust extension events ready for emission.
    pub events: Vec<HermesSpaceTrustExtension>,
    /// Count of verified additions.
    pub verified: u64,
    /// Count of related additions.
    pub related: u64,
    /// Count of topic declarations.
    pub topic_declared: u64,
    /// Count of verified removals.
    pub unverified: u64,
    /// Count of related removals.
    pub unrelated: u64,
    /// Count of topic removals.
    pub topic_removed: u64,
}

impl TransformResult {
    /// Total number of trust events. Used to gate emission — if zero, the
    /// entire trust section is skipped for this block.
    pub fn total(&self) -> u64 {
        self.verified
            + self.related
            + self.topic_declared
            + self.unverified
            + self.unrelated
            + self.topic_removed
    }
}

/// Transform all trust-related actions in a block.
///
/// Handles the following action types:
/// - SUBSPACE_VERIFIED: Verified trust extension
/// - SUBSPACE_RELATED: Related trust extension
/// - SUBSPACE_TOPIC_SET: Topic-based trust extension
/// - SUBSPACE_UNVERIFIED: Verified trust removal
/// - SUBSPACE_UNRELATED: Related trust removal
/// - SUBSPACE_TOPIC_UNSET: Topic trust removal
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
                target = %hex::encode(&action.topic[0..16])
            )
            .in_scope(|| convert_verified(action, meta, sequence))?;
            result.events.push(event);
            result.verified += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_RELATED) {
            let event = debug_span!(
                "convert.trust.related",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[0..16])
            )
            .in_scope(|| convert_related(action, meta, sequence))?;
            result.events.push(event);
            result.related += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_TOPIC_SET) {
            let event = debug_span!(
                "convert.trust.topic_declared",
                source = %hex::encode(&action.from_id),
                subspace = %hex::encode(&action.topic[0..16]),
                topic = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_topic_declared(action, meta, sequence))?;
            result.events.push(event);
            result.topic_declared += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_UNVERIFIED) {
            let event = debug_span!(
                "convert.trust.unverified",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[0..16])
            )
            .in_scope(|| convert_unverified(action, meta, sequence))?;
            result.events.push(event);
            result.unverified += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_UNRELATED) {
            let event = debug_span!(
                "convert.trust.unrelated",
                source = %hex::encode(&action.from_id),
                target = %hex::encode(&action.topic[0..16])
            )
            .in_scope(|| convert_unrelated(action, meta, sequence))?;
            result.events.push(event);
            result.unrelated += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_TOPIC_UNSET) {
            let event = debug_span!(
                "convert.trust.topic_removed",
                source = %hex::encode(&action.from_id),
                topic = %hex::encode(&action.topic[16..32])
            )
            .in_scope(|| convert_topic_removed(action, meta, sequence))?;
            result.events.push(event);
            result.topic_removed += 1;
        }
    }

    Ok(result)
}

/// Convert a SUBSPACE_VERIFIED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_VERIFIED:
/// - from_id: parent_space_id (16 bytes)
/// - topic: [subspace_id (16 bytes)][padding (16 bytes)]
///
/// ZC16: Solidity `bytes32(bytes16)` right-pads, so the bytes16 value is in [0..16].
fn convert_verified(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[0..16].to_vec();

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
/// - topic: [subspace_id (16 bytes)][padding (16 bytes)]
///
/// ZC16: Solidity `bytes32(bytes16)` right-pads, so the bytes16 value is in [0..16].
fn convert_related(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[0..16].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::Related(
        RelatedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_TOPIC_SET action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_TOPIC_SET:
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

/// Convert a SUBSPACE_UNVERIFIED action to HermesSpaceTrustExtension proto.
///
/// action.topic layout: [target_space_id: 16 bytes | padding: 16 bytes]
///
/// ZC16: Solidity `bytes32(bytes16)` right-pads, so the bytes16 value is in [0..16].
fn convert_unverified(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[0..16].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::VerifiedRemoval(
        VerifiedRemoval { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_UNRELATED action to HermesSpaceTrustExtension proto.
///
/// action.topic layout: [target_space_id: 16 bytes | padding: 16 bytes]
///
/// ZC16: Solidity `bytes32(bytes16)` right-pads, so the bytes16 value is in [0..16].
fn convert_unrelated(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.topic[0..16].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::RelatedRemoval(
        RelatedRemoval { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SUBSPACE_TOPIC_UNSET action to HermesSpaceTrustExtension proto.
///
/// action.topic layout: [subspace_id: 16 bytes | topic_id: 16 bytes]
fn convert_topic_removed(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_topic_id = action.topic[16..32].to_vec();

    let extension = Some(hermes_space_trust_extension::Extension::SubtopicRemoval(
        SubtopicRemoval { target_topic_id },
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
        Some(hermes_space_trust_extension::Extension::VerifiedRemoval(_)) => "verified_removal",
        Some(hermes_space_trust_extension::Extension::RelatedRemoval(_)) => "related_removal",
        Some(hermes_space_trust_extension::Extension::SubtopicRemoval(_)) => "subtopic_removal",
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
        // ZC16: bytes32(bytes16) right-pads — target in [0..16], zeros in [16..32]
        let mut topic = target.to_vec();
        topic.extend_from_slice(&[0u8; 16]); // padding
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
            action: actions::SUBSPACE_TOPIC_SET.to_vec(),
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
    fn test_convert_subspace_unverified() {
        let target = vec![2u8; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_UNVERIFIED.to_vec(),
            topic: make_topic_with_target(&target),
            data: vec![],
        };

        let result = convert_unverified(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::VerifiedRemoval(v)) => {
                assert_eq!(v.target_space_id, target);
            }
            _ => panic!("Expected VerifiedRemoval extension"),
        }
    }

    #[test]
    fn test_convert_subspace_unrelated() {
        let target = vec![2u8; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_UNRELATED.to_vec(),
            topic: make_topic_with_target(&target),
            data: vec![],
        };

        let result = convert_unrelated(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::RelatedRemoval(r)) => {
                assert_eq!(r.target_space_id, target);
            }
            _ => panic!("Expected RelatedRemoval extension"),
        }
    }

    #[test]
    fn test_convert_topic_removed() {
        let subspace = vec![2u8; 16];
        let topic_id = vec![3u8; 16];
        let mut topic = subspace.clone();
        topic.extend_from_slice(&topic_id);

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_TOPIC_UNSET.to_vec(),
            topic,
            data: vec![],
        };

        let result = convert_topic_removed(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        match result.extension {
            Some(hermes_space_trust_extension::Extension::SubtopicRemoval(s)) => {
                assert_eq!(s.target_topic_id, topic_id);
            }
            _ => panic!("Expected SubtopicRemoval extension"),
        }
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
                action: actions::SUBSPACE_TOPIC_SET.to_vec(),
                topic: {
                    let mut t = vec![6u8; 16];
                    t.extend_from_slice(&[7u8; 16]);
                    t
                },
                data: vec![],
            },
            Action {
                from_id: vec![10; 16],
                to_id: vec![],
                action: actions::SUBSPACE_UNVERIFIED.to_vec(),
                topic: make_topic_with_target(&[11u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![12; 16],
                to_id: vec![],
                action: actions::SUBSPACE_UNRELATED.to_vec(),
                topic: make_topic_with_target(&[13u8; 16]),
                data: vec![],
            },
            Action {
                from_id: vec![14; 16],
                to_id: vec![],
                action: actions::SUBSPACE_TOPIC_UNSET.to_vec(),
                topic: {
                    let mut t = vec![15u8; 16];
                    t.extend_from_slice(&[16u8; 16]);
                    t
                },
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 6);
        assert_eq!(result.verified, 1);
        assert_eq!(result.related, 1);
        assert_eq!(result.topic_declared, 1);
        assert_eq!(result.unverified, 1);
        assert_eq!(result.unrelated, 1);
        assert_eq!(result.topic_removed, 1);
        assert_eq!(result.total(), 6);
    }

    /// Blocks with only removal events must still have total() > 0
    /// (otherwise the emission gate at main.rs skips them).
    #[test]
    fn test_total_includes_removal_only_block() {
        let actions = vec![Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SUBSPACE_UNVERIFIED.to_vec(),
            topic: make_topic_with_target(&[2u8; 16]),
            data: vec![],
        }];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 1);
        assert!(result.total() > 0, "removal-only block must have total > 0");
    }
}
