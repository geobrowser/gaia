//! Pipeline: SPACE_REGISTERED → space.creations
//!
//! Converts space registration actions to HermesCreateSpace events.

use anyhow::Result;

use hermes_relay::{actions, Action};
use hermes_schema::pb::space::{
    hermes_create_space, DefaultDaoSpacePayload, HermesCreateSpace, PersonalSpacePayload,
};

use super::BlockMetadata;

/// Result of transforming space actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed space creation events ready for emission.
    pub events: Vec<HermesCreateSpace>,
}

/// Transform all SPACE_REGISTERED actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut events = Vec::new();

    for action in actions {
        if !actions::matches(&action.action, &actions::SPACE_REGISTERED) {
            continue;
        }

        let event = convert(action, meta)?;
        events.push(event);
    }

    Ok(TransformResult { events })
}

/// Convert a SPACE_REGISTERED action to HermesCreateSpace proto.
///
/// The action structure for SPACE_REGISTERED:
/// - from_id: space_id (16 bytes)
/// - to_id: space_id (16 bytes, same as from_id)
/// - topic: space_address (20 bytes, padded to 32)
/// - data: encoded space creation payload
fn convert(action: &Action, meta: &BlockMetadata) -> Result<HermesCreateSpace> {
    let space_id = action.from_id.clone();

    // Determine space type from data field
    // Empty data = Personal space, non-empty = DAO space with members
    let payload = if action.data.is_empty() {
        Some(hermes_create_space::Payload::PersonalSpace(
            PersonalSpacePayload {
                owner: action.topic.clone(),
            },
        ))
    } else {
        // DAO space - for now use empty lists (full decoding would parse data field)
        Some(hermes_create_space::Payload::DefaultDaoSpace(
            DefaultDaoSpacePayload {
                initial_editors: vec![],
                initial_members: vec![],
            },
        ))
    };

    Ok(HermesCreateSpace {
        space_id,
        topic_id: action.topic.clone(),
        payload,
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
    fn test_convert_personal_space() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![1; 16],
            action: actions::SPACE_REGISTERED.to_vec(),
            topic: vec![2; 32],
            data: vec![], // Empty = personal space
        };

        let result = convert(&action, &test_meta()).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert!(matches!(
            result.payload,
            Some(hermes_create_space::Payload::PersonalSpace(_))
        ));
    }

    #[test]
    fn test_convert_dao_space() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![1; 16],
            action: actions::SPACE_REGISTERED.to_vec(),
            topic: vec![2; 32],
            data: vec![1, 2, 3], // Non-empty = DAO space
        };

        let result = convert(&action, &test_meta()).unwrap();
        assert!(matches!(
            result.payload,
            Some(hermes_create_space::Payload::DefaultDaoSpace(_))
        ));
    }

    #[test]
    fn test_transform_filters_actions() {
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![1; 16],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![2; 32],
                data: vec![],
            },
            Action {
                from_id: vec![2; 16],
                to_id: vec![3; 16],
                action: actions::SUBSPACE_ADDED.to_vec(), // Different action type
                topic: vec![3; 32],
                data: vec![],
            },
            Action {
                from_id: vec![4; 16],
                to_id: vec![4; 16],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![5; 32],
                data: vec![1, 2, 3],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 2); // Only SPACE_REGISTERED actions
    }
}
