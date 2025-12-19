//! Pipeline: SPACE_REGISTERED → space.creations
//!
//! Converts space registration actions to HermesCreateSpace events.

use anyhow::Result;
use hermes_instrumentation::debug_span;

use hermes_relay::{Action, actions};
use hermes_schema::pb::space::{
    DefaultDaoSpacePayload, HermesCreateSpace, PersonalSpacePayload, hermes_create_space,
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

    for (index, action) in actions.iter().enumerate() {
        if !actions::matches(&action.action, &actions::SPACE_REGISTERED) {
            continue;
        }

        let sequence = index as u32;
        let event = debug_span!("convert.space", space_id = %hex::encode(&action.from_id))
            .in_scope(|| convert(action, meta, sequence))?;
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
/// - data: encoded space creation payload (empty for personal, encoded editors/members for DAO)
fn convert(action: &Action, meta: &BlockMetadata, sequence: u32) -> Result<HermesCreateSpace> {
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
        let (initial_editors, initial_members) = decode_dao_data(&action.data)?;
        Some(hermes_create_space::Payload::DefaultDaoSpace(
            DefaultDaoSpacePayload {
                initial_editors,
                initial_members,
            },
        ))
    };

    Ok(HermesCreateSpace {
        space_id,
        topic_id: action.topic.clone(),
        payload,
        meta: Some(meta.to_proto(sequence)),
    })
}

/// Decode DAO space data field into initial editors and members.
///
/// Format:
/// - 2 bytes: number of editors (u16 big-endian)
/// - N * 16 bytes: editor space IDs
/// - 2 bytes: number of members (u16 big-endian)
/// - M * 16 bytes: member space IDs
#[allow(clippy::type_complexity)]
fn decode_dao_data(data: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let mut offset = 0;

    // Decode editors
    if data.len() < offset + 2 {
        anyhow::bail!("DAO data too short for editor count");
    }
    let editor_count = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;

    let mut initial_editors = Vec::with_capacity(editor_count);
    for _ in 0..editor_count {
        if data.len() < offset + 16 {
            anyhow::bail!("DAO data too short for editor ID");
        }
        initial_editors.push(data[offset..offset + 16].to_vec());
        offset += 16;
    }

    // Decode members
    if data.len() < offset + 2 {
        anyhow::bail!("DAO data too short for member count");
    }
    let member_count = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;

    let mut initial_members = Vec::with_capacity(member_count);
    for _ in 0..member_count {
        if data.len() < offset + 16 {
            anyhow::bail!("DAO data too short for member ID");
        }
        initial_members.push(data[offset..offset + 16].to_vec());
        offset += 16;
    }

    Ok((initial_editors, initial_members))
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

        let result = convert(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert!(matches!(
            result.payload,
            Some(hermes_create_space::Payload::PersonalSpace(_))
        ));
    }

    #[test]
    fn test_convert_dao_space() {
        // Encode 1 editor and 0 members
        let mut data = Vec::new();
        data.extend_from_slice(&1u16.to_be_bytes()); // 1 editor
        data.extend_from_slice(&[0xAA; 16]); // editor ID
        data.extend_from_slice(&0u16.to_be_bytes()); // 0 members

        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![1; 16],
            action: actions::SPACE_REGISTERED.to_vec(),
            topic: vec![2; 32],
            data,
        };

        let result = convert(&action, &test_meta(), 0).unwrap();
        match result.payload {
            Some(hermes_create_space::Payload::DefaultDaoSpace(dao)) => {
                assert_eq!(dao.initial_editors.len(), 1);
                assert_eq!(dao.initial_editors[0], vec![0xAA; 16]);
                assert_eq!(dao.initial_members.len(), 0);
            }
            _ => panic!("Expected DefaultDaoSpace payload"),
        }
    }

    #[test]
    fn test_decode_dao_data() {
        // Encode 2 editors and 1 member
        let mut data = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes()); // 2 editors
        data.extend_from_slice(&[0x11; 16]); // editor 1
        data.extend_from_slice(&[0x22; 16]); // editor 2
        data.extend_from_slice(&1u16.to_be_bytes()); // 1 member
        data.extend_from_slice(&[0x33; 16]); // member 1

        let (editors, members) = decode_dao_data(&data).unwrap();

        assert_eq!(editors.len(), 2);
        assert_eq!(editors[0], vec![0x11; 16]);
        assert_eq!(editors[1], vec![0x22; 16]);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0], vec![0x33; 16]);
    }

    #[test]
    fn test_decode_dao_data_empty() {
        // Encode 0 editors and 0 members
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_be_bytes()); // 0 editors
        data.extend_from_slice(&0u16.to_be_bytes()); // 0 members

        let (editors, members) = decode_dao_data(&data).unwrap();

        assert_eq!(editors.len(), 0);
        assert_eq!(members.len(), 0);
    }

    #[test]
    fn test_transform_filters_actions() {
        // Valid DAO data: 0 editors, 0 members
        let mut dao_data = Vec::new();
        dao_data.extend_from_slice(&0u16.to_be_bytes());
        dao_data.extend_from_slice(&0u16.to_be_bytes());

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
                data: dao_data,
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.events.len(), 2); // Only SPACE_REGISTERED actions
    }
}
