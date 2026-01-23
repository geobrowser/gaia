//! Pipeline: SPACE_FAST_PATH_RESTRICTED, SPACE_FAST_PATH_UNRESTRICTED, FLAGGED, UNFLAGGED → space.moderation
//!
//! Converts moderation actions to typed Hermes events with decoded data.

use anyhow::Result;
use hermes_instrumentation::{debug_span, warn};

use hermes_codec::actions;
use hermes_relay::Action;
use hermes_schema::pb::moderation::{
    HermesContentFlagged, HermesContentUnflagged, HermesEditorFlagged, HermesEditorUnflagged,
};

use anyhow::Context;

use hermes_codec as decode;

use super::BlockMetadata;

/// Result of transforming moderation actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub editors_flagged: Vec<HermesEditorFlagged>,
    pub editors_unflagged: Vec<HermesEditorUnflagged>,
    pub content_flagged: Vec<HermesContentFlagged>,
    pub content_unflagged: Vec<HermesContentUnflagged>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.editors_flagged.len()
            + self.editors_unflagged.len()
            + self.content_flagged.len()
            + self.content_unflagged.len()
    }
}

/// Transform all moderation actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::SPACE_FAST_PATH_RESTRICTED) {
            let event = debug_span!(
                "convert.moderation.editor_flagged",
                space_id = %hex::encode(&action.from_id),
                editor = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_editor_flagged(action, meta, sequence))?;
            result.editors_flagged.push(event);
        } else if actions::matches(action_type, &actions::SPACE_FAST_PATH_UNRESTRICTED) {
            let event = debug_span!(
                "convert.moderation.editor_unflagged",
                space_id = %hex::encode(&action.from_id),
                editor = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_editor_unflagged(action, meta, sequence))?;
            result.editors_unflagged.push(event);
        } else if actions::matches(action_type, &actions::FLAGGED) {
            let event = debug_span!(
                "convert.moderation.content_flagged",
                flagger_id = %hex::encode(&action.from_id),
                target_space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| convert_content_flagged(action, meta, sequence))?;
            result.content_flagged.push(event);
        } else if actions::matches(action_type, &actions::UNFLAGGED) {
            let event = debug_span!(
                "convert.moderation.content_unflagged",
                unflagger_id = %hex::encode(&action.from_id),
                target_space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| convert_content_unflagged(action, meta, sequence))?;
            result.content_unflagged.push(event);
        }
    }

    Ok(result)
}

/// Convert a SPACE_FAST_PATH_RESTRICTED action to HermesEditorFlagged proto.
///
/// ZC16 action structure:
/// - from_id: space_id (16 bytes) - DAO space where action occurs
/// - topic: bytes32(editorSpaceId) - editor's space ID (first 16 bytes)
/// - data: abi.encode(address) - editor's account address
fn convert_editor_flagged(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesEditorFlagged> {
    // ZC16: Extract the 20-byte address from the ABI-encoded data field
    let editor_account = decode::decode_address(&action.data)
        .context("Failed to decode editor address from data field")?;

    // Extract editor's space ID from first 16 bytes of topic
    let editor_space_id = action.topic[..16].to_vec();

    Ok(HermesEditorFlagged {
        space_id: action.from_id.clone(),
        editor_account,
        editor_space_id,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a SPACE_FAST_PATH_UNRESTRICTED action to HermesEditorUnflagged proto.
///
/// ZC16 action structure:
/// - from_id: space_id (16 bytes) - DAO space where action occurs
/// - topic: bytes32(editorSpaceId) - editor's space ID (first 16 bytes)
/// - data: abi.encode(address) - editor's account address
fn convert_editor_unflagged(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesEditorUnflagged> {
    // ZC16: Extract the 20-byte address from the ABI-encoded data field
    let editor_account = decode::decode_address(&action.data)
        .context("Failed to decode editor address from data field")?;

    // Extract editor's space ID from first 16 bytes of topic
    let editor_space_id = action.topic[..16].to_vec();

    Ok(HermesEditorUnflagged {
        space_id: action.from_id.clone(),
        editor_account,
        editor_space_id,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert a FLAGGED action to HermesContentFlagged proto.
///
/// The action structure:
/// - from_id: flagger_id (16 bytes) - space flagging content
/// - to_id: target_space_id (16 bytes) - space whose content is flagged
/// - topic: topic_id (32 bytes) - optional topic UUID
/// - data: abi.encode(bytes(flaggedUri))
fn convert_content_flagged(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesContentFlagged> {
    // Decode the URI from the data field
    let uri = match decode::decode_flag_data(&action.data) {
        Ok(decoded_uri) => decoded_uri,
        Err(e) => {
            warn!(
                error = %e,
                flagger_id = %hex::encode(&action.from_id),
                "Failed to decode content flagged data"
            );
            String::new()
        }
    };

    Ok(HermesContentFlagged {
        flagger_id: action.from_id.clone(),
        target_space_id: action.to_id.clone(),
        topic_id: action.topic.clone(),
        uri,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Convert an UNFLAGGED action to HermesContentUnflagged proto.
///
/// The action structure:
/// - from_id: unflagger_id (16 bytes) - space unflagging content
/// - to_id: target_space_id (16 bytes) - space whose content is unflagged
/// - topic: topic_id (32 bytes) - optional topic UUID
/// - data: abi.encode(bytes(unflaggedUri))
fn convert_content_unflagged(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesContentUnflagged> {
    // Decode the URI from the data field
    let uri = match decode::decode_flag_data(&action.data) {
        Ok(decoded_uri) => decoded_uri,
        Err(e) => {
            warn!(
                error = %e,
                unflagger_id = %hex::encode(&action.from_id),
                "Failed to decode content unflagged data"
            );
            String::new()
        }
    };

    Ok(HermesContentUnflagged {
        unflagger_id: action.from_id.clone(),
        target_space_id: action.to_id.clone(),
        topic_id: action.topic.clone(),
        uri,
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
    fn test_convert_editor_flagged() {
        // ZC16 format: editor space ID in topic (first 16 bytes), address in data
        let editor_space_id = vec![3; 16];
        let mut topic = editor_space_id.clone();
        topic.extend(vec![0; 16]); // Pad to 32 bytes

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::SPACE_FAST_PATH_RESTRICTED.to_vec(),
            topic,
            data: vec![0; 12].into_iter().chain(vec![2; 20]).collect(), // ABI-encoded address
        };

        let result = convert_editor_flagged(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.editor_account, vec![2; 20]);
        assert_eq!(result.editor_space_id, vec![3; 16]);
    }

    #[test]
    fn test_convert_content_flagged_empty_data() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::FLAGGED.to_vec(),
            topic: vec![3; 32],
            data: vec![],
        };

        let result = convert_content_flagged(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.flagger_id, vec![1; 16]);
        assert_eq!(result.target_space_id, vec![2; 16]);
        assert!(result.uri.is_empty());
    }

    #[test]
    fn test_convert_content_unflagged_empty_data() {
        let action = Action {
            from_id: vec![4; 16],
            to_id: vec![5; 16],
            action: actions::UNFLAGGED.to_vec(),
            topic: vec![6; 32],
            data: vec![],
        };

        let result = convert_content_unflagged(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.unflagger_id, vec![4; 16]);
        assert_eq!(result.target_space_id, vec![5; 16]);
        assert_eq!(result.topic_id, vec![6; 32]);
        assert!(result.uri.is_empty());
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            // ZC16 format: editor space ID in topic, address in data
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::SPACE_FAST_PATH_RESTRICTED.to_vec(),
                topic: vec![2; 16].into_iter().chain(vec![0; 16]).collect(),
                data: vec![0; 12].into_iter().chain(vec![7; 20]).collect(), // ABI-encoded address
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![4; 16],
                action: actions::FLAGGED.to_vec(),
                topic: vec![0; 32],
                data: vec![],
            },
            Action {
                from_id: vec![5; 16],
                to_id: vec![6; 16],
                action: actions::UNFLAGGED.to_vec(),
                topic: vec![0; 32],
                data: vec![],
            },
            // Should NOT be included
            Action {
                from_id: vec![8; 16],
                to_id: vec![],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![9; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.editors_flagged.len(), 1);
        assert_eq!(result.editors_unflagged.len(), 0);
        assert_eq!(result.content_flagged.len(), 1);
        assert_eq!(result.content_unflagged.len(), 1);
        assert_eq!(result.total(), 3);
    }
}
