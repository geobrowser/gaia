//! Pipeline: SUBSPACE_ADDED/REMOVED → space.trust.extensions
//!
//! Converts trust extension and revocation actions to HermesSpaceTrustExtension events.

use anyhow::Result;

use hermes_relay::{actions, Action};
use hermes_schema::pb::space::{
    hermes_space_trust_extension, HermesSpaceTrustExtension, VerifiedExtension,
};

use super::BlockMetadata;

/// A trust event with its type (added or removed).
#[derive(Debug, Clone)]
pub struct TrustEvent {
    pub event: HermesSpaceTrustExtension,
    pub is_removal: bool,
}

/// Result of transforming trust actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed trust extension events ready for emission.
    pub events: Vec<TrustEvent>,
    /// Count of additions.
    pub added: u64,
    /// Count of removals.
    pub removed: u64,
}

/// Transform all SUBSPACE_ADDED and SUBSPACE_REMOVED actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut events = Vec::new();
    let mut added = 0u64;
    let mut removed = 0u64;

    for action in actions {
        let action_type = action.action.as_slice();

        if actions::matches(action_type, &actions::SUBSPACE_ADDED) {
            let event = convert_added(action, meta)?;
            events.push(TrustEvent {
                event,
                is_removal: false,
            });
            added += 1;
        } else if actions::matches(action_type, &actions::SUBSPACE_REMOVED) {
            let event = convert_removed(action, meta)?;
            events.push(TrustEvent {
                event,
                is_removal: true,
            });
            removed += 1;
        }
    }

    Ok(TransformResult {
        events,
        added,
        removed,
    })
}

/// Convert a SUBSPACE_ADDED action to HermesSpaceTrustExtension proto.
///
/// The action structure for SUBSPACE_ADDED:
/// - from_id: parent_space_id (16 bytes)
/// - to_id: subspace_id (16 bytes)
/// - topic: subspace_id padded to 32 bytes
/// - data: encoded trust type and metadata
fn convert_added(action: &Action, meta: &BlockMetadata) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.to_id.clone();

    // Default to Verified extension type
    // In a full implementation, we'd decode the data field to determine the type
    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto()),
    })
}

/// Convert a SUBSPACE_REMOVED action to HermesSpaceTrustExtension proto.
///
/// Uses the same structure as SUBSPACE_ADDED but represents a trust revocation.
fn convert_removed(action: &Action, meta: &BlockMetadata) -> Result<HermesSpaceTrustExtension> {
    let source_space_id = action.from_id.clone();
    let target_space_id = action.to_id.clone();

    let extension = Some(hermes_space_trust_extension::Extension::Verified(
        VerifiedExtension { target_space_id },
    ));

    Ok(HermesSpaceTrustExtension {
        source_space_id,
        extension,
        meta: Some(meta.to_proto()),
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

    #[test]
    fn test_convert_subspace_added() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::SUBSPACE_ADDED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_added(&action, &test_meta()).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
        assert!(matches!(
            result.extension,
            Some(hermes_space_trust_extension::Extension::Verified(_))
        ));
    }

    #[test]
    fn test_convert_subspace_removed() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::SUBSPACE_REMOVED.to_vec(),
            topic: vec![2; 32],
            data: vec![],
        };

        let result = convert_removed(&action, &test_meta()).unwrap();
        assert_eq!(result.source_space_id, vec![1; 16]);
    }

    #[test]
    fn test_transform_counts() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![2; 16],
                action: actions::SUBSPACE_ADDED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![4; 16],
                action: actions::SUBSPACE_ADDED.to_vec(),
                topic: vec![4; 32],
                data: vec![],
            },
            Action {
                from_id: vec![5; 16],
                to_id: vec![6; 16],
                action: actions::SUBSPACE_REMOVED.to_vec(),
                topic: vec![6; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 3);
        assert_eq!(result.added, 2);
        assert_eq!(result.removed, 1);
    }
}
