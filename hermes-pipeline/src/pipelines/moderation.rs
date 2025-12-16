//! Pipeline: EDITOR_FLAGGED, EDITOR_UNFLAGGED, FLAGGED, UNFLAGGED → space.moderation
//!
//! Converts moderation actions to typed Hermes events.

use anyhow::Result;
use hermes_instrumentation::debug_span;

use hermes_relay::{Action, actions};
use hermes_schema::pb::moderation::{
    HermesContentFlagged, HermesContentUnflagged, HermesEditorFlagged, HermesEditorUnflagged,
};

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

    for action in actions {
        let action_type = action.action.as_slice();

        if actions::matches(action_type, &actions::EDITOR_FLAGGED) {
            let event = debug_span!(
                "convert.moderation.editor_flagged",
                space_id = %hex::encode(&action.from_id),
                editor = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_editor_flagged(action, meta))?;
            result.editors_flagged.push(event);
        } else if actions::matches(action_type, &actions::EDITOR_UNFLAGGED) {
            let event = debug_span!(
                "convert.moderation.editor_unflagged",
                space_id = %hex::encode(&action.from_id),
                editor = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_editor_unflagged(action, meta))?;
            result.editors_unflagged.push(event);
        } else if actions::matches(action_type, &actions::FLAGGED) {
            let event = debug_span!(
                "convert.moderation.content_flagged",
                flagger_id = %hex::encode(&action.from_id),
                target_space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| convert_content_flagged(action, meta))?;
            result.content_flagged.push(event);
        } else if actions::matches(action_type, &actions::UNFLAGGED) {
            let event = debug_span!(
                "convert.moderation.content_unflagged",
                unflagger_id = %hex::encode(&action.from_id),
                target_space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| convert_content_unflagged(action, meta))?;
            result.content_unflagged.push(event);
        }
    }

    Ok(result)
}

/// Convert an EDITOR_FLAGGED action to HermesEditorFlagged proto.
///
/// The action structure:
/// - from_id: space_id (16 bytes) - space containing editor
/// - topic: editor address (32 bytes, padded from 20)
/// - data: flag details/reason
fn convert_editor_flagged(action: &Action, meta: &BlockMetadata) -> Result<HermesEditorFlagged> {
    // Extract the 20-byte address from the 32-byte topic (last 20 bytes)
    let editor_account = if action.topic.len() >= 20 {
        action.topic[action.topic.len() - 20..].to_vec()
    } else {
        action.topic.clone()
    };

    Ok(HermesEditorFlagged {
        space_id: action.from_id.clone(),
        editor_account,
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

/// Convert an EDITOR_UNFLAGGED action to HermesEditorUnflagged proto.
///
/// The action structure:
/// - from_id: space_id (16 bytes) - space containing editor
/// - topic: editor address (32 bytes, padded from 20)
/// - data: unflag details
fn convert_editor_unflagged(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesEditorUnflagged> {
    let editor_account = if action.topic.len() >= 20 {
        action.topic[action.topic.len() - 20..].to_vec()
    } else {
        action.topic.clone()
    };

    Ok(HermesEditorUnflagged {
        space_id: action.from_id.clone(),
        editor_account,
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

/// Convert a FLAGGED action to HermesContentFlagged proto.
///
/// The action structure:
/// - from_id: flagger_id (16 bytes) - space flagging content
/// - to_id: target_space_id (16 bytes) - space whose content is flagged
/// - data: flag details (entity type, ID, reason)
fn convert_content_flagged(action: &Action, meta: &BlockMetadata) -> Result<HermesContentFlagged> {
    Ok(HermesContentFlagged {
        flagger_id: action.from_id.clone(),
        target_space_id: action.to_id.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
    })
}

/// Convert an UNFLAGGED action to HermesContentUnflagged proto.
///
/// The action structure:
/// - from_id: unflagger_id (16 bytes) - space unflagging content
/// - to_id: target_space_id (16 bytes) - space whose content is unflagged
/// - data: unflag details
fn convert_content_unflagged(
    action: &Action,
    meta: &BlockMetadata,
) -> Result<HermesContentUnflagged> {
    Ok(HermesContentUnflagged {
        unflagger_id: action.from_id.clone(),
        target_space_id: action.to_id.clone(),
        data: action.data.clone(),
        meta: Some(meta.to_proto()),
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
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::EDITOR_FLAGGED.to_vec(),
            topic: vec![0; 12].into_iter().chain(vec![2; 20]).collect(),
            data: vec![3, 4, 5],
        };

        let result = convert_editor_flagged(&action, &test_meta()).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.editor_account, vec![2; 20]);
        assert_eq!(result.data, vec![3, 4, 5]);
    }

    #[test]
    fn test_convert_content_flagged() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::FLAGGED.to_vec(),
            topic: vec![0; 32],
            data: b"ipfs://QmTest".to_vec(),
        };

        let result = convert_content_flagged(&action, &test_meta()).unwrap();
        assert_eq!(result.flagger_id, vec![1; 16]);
        assert_eq!(result.target_space_id, vec![2; 16]);
        assert_eq!(result.data, b"ipfs://QmTest".to_vec());
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::EDITOR_FLAGGED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
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
