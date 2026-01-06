//! Pipeline: EDITOR_ADDED, EDITOR_REMOVED, MEMBER_ADDED, MEMBER_REMOVED, SPACE_LEFT → space.membership
//!
//! Converts membership actions to typed Hermes events.

use anyhow::{Context, Result};
use hermes_instrumentation::debug_span;

use hermes_relay::{Action, actions};
use hermes_schema::pb::membership::{
    HermesRoleGranted, HermesRoleRevoked, HermesSpaceLeft, MembershipRole,
};

use super::BlockMetadata;
use crate::decode::decode_address;

/// Result of transforming membership actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    pub roles_granted: Vec<HermesRoleGranted>,
    pub roles_revoked: Vec<HermesRoleRevoked>,
    pub spaces_left: Vec<HermesSpaceLeft>,
}

impl TransformResult {
    pub fn total(&self) -> usize {
        self.roles_granted.len() + self.roles_revoked.len() + self.spaces_left.len()
    }
}

/// Transform all membership actions in a block.
///
/// Returns transformed events without sending to Kafka.
pub fn transform(actions: &[Action], meta: &BlockMetadata) -> Result<TransformResult> {
    let mut result = TransformResult::default();

    for (index, action) in actions.iter().enumerate() {
        let action_type = action.action.as_slice();
        let sequence = index as u32;

        if actions::matches(action_type, &actions::EDITOR_ADDED) {
            let event = debug_span!(
                "convert.membership.editor_added",
                space_id = %hex::encode(&action.from_id),
                account = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_role_granted(action, meta, MembershipRole::Editor, sequence))?;
            result.roles_granted.push(event);
        } else if actions::matches(action_type, &actions::EDITOR_REMOVED) {
            let event = debug_span!(
                "convert.membership.editor_removed",
                space_id = %hex::encode(&action.from_id),
                account = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_role_revoked(action, meta, MembershipRole::Editor, sequence))?;
            result.roles_revoked.push(event);
        } else if actions::matches(action_type, &actions::MEMBER_ADDED) {
            let event = debug_span!(
                "convert.membership.member_added",
                space_id = %hex::encode(&action.from_id),
                account = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_role_granted(action, meta, MembershipRole::Member, sequence))?;
            result.roles_granted.push(event);
        } else if actions::matches(action_type, &actions::MEMBER_REMOVED) {
            let event = debug_span!(
                "convert.membership.member_removed",
                space_id = %hex::encode(&action.from_id),
                account = %hex::encode(&action.topic)
            )
            .in_scope(|| convert_role_revoked(action, meta, MembershipRole::Member, sequence))?;
            result.roles_revoked.push(event);
        } else if actions::matches(action_type, &actions::SPACE_LEFT) {
            let event = debug_span!(
                "convert.membership.space_left",
                member_id = %hex::encode(&action.from_id),
                space_id = %hex::encode(&action.to_id)
            )
            .in_scope(|| convert_space_left(action, meta, sequence))?;
            result.spaces_left.push(event);
        }
    }

    Ok(result)
}

/// Convert an EDITOR_ADDED or MEMBER_ADDED action to HermesRoleGranted proto.
///
/// ZC16 action structure:
/// - from_id: space_id (16 bytes) - space granting role
/// - topic: bytes32(memberSpaceId) - member's space ID (first 16 bytes)
/// - data: abi.encode(address) - 32 bytes with address in last 20 bytes
fn convert_role_granted(
    action: &Action,
    meta: &BlockMetadata,
    role: MembershipRole,
    sequence: u32,
) -> Result<HermesRoleGranted> {
    // ZC16: Extract the 20-byte address from the ABI-encoded data field
    let account = decode_address(&action.data)
        .context("Failed to decode account address from data field")?;

    // ZC16: Extract the member's space ID from the first 16 bytes of topic
    let member_space_id = action.topic.get(..16).unwrap_or_default().to_vec();

    Ok(HermesRoleGranted {
        space_id: action.from_id.clone(),
        account,
        role: role as i32,
        meta: Some(meta.to_proto(sequence)),
        member_space_id,
    })
}

/// Convert an EDITOR_REMOVED or MEMBER_REMOVED action to HermesRoleRevoked proto.
///
/// ZC16 action structure:
/// - from_id: space_id (16 bytes) - space revoking role
/// - topic: bytes32(memberSpaceId) - member's space ID (first 16 bytes)
/// - data: abi.encode(address) - 32 bytes with address in last 20 bytes
fn convert_role_revoked(
    action: &Action,
    meta: &BlockMetadata,
    role: MembershipRole,
    sequence: u32,
) -> Result<HermesRoleRevoked> {
    // ZC16: Extract the 20-byte address from the ABI-encoded data field
    let account = decode_address(&action.data)
        .context("Failed to decode account address from data field")?;

    // ZC16: Extract the member's space ID from the first 16 bytes of topic
    let member_space_id = action.topic.get(..16).unwrap_or_default().to_vec();

    Ok(HermesRoleRevoked {
        space_id: action.from_id.clone(),
        account,
        role: role as i32,
        meta: Some(meta.to_proto(sequence)),
        member_space_id,
    })
}

/// Convert a SPACE_LEFT action to HermesSpaceLeft proto.
///
/// The action structure:
/// - from_id: member_id (16 bytes) - member leaving
/// - to_id: space_id (16 bytes) - space being left
fn convert_space_left(
    action: &Action,
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesSpaceLeft> {
    Ok(HermesSpaceLeft {
        member_id: action.from_id.clone(),
        space_id: action.to_id.clone(),
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
    fn test_convert_editor_added() {
        // ZC16 format: topic contains member's space ID, data contains ABI-encoded address
        let member_space_id: Vec<u8> = vec![0xAB; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::EDITOR_ADDED.to_vec(),
            topic: member_space_id.iter().copied().chain(vec![0; 16]).collect(), // bytes32(memberSpaceId)
            data: vec![0; 12].into_iter().chain(vec![2; 20]).collect(), // ABI-encoded address
        };

        let result =
            convert_role_granted(&action, &test_meta(), MembershipRole::Editor, 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.account, vec![2; 20]);
        assert_eq!(result.role, MembershipRole::Editor as i32);
        assert_eq!(result.member_space_id, member_space_id);
    }

    #[test]
    fn test_convert_member_removed() {
        // ZC16 format: topic contains member's space ID, data contains ABI-encoded address
        let member_space_id: Vec<u8> = vec![0xCD; 16];
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![],
            action: actions::MEMBER_REMOVED.to_vec(),
            topic: member_space_id.iter().copied().chain(vec![0; 16]).collect(), // bytes32(memberSpaceId)
            data: vec![0; 12].into_iter().chain(vec![3; 20]).collect(), // ABI-encoded address
        };

        let result =
            convert_role_revoked(&action, &test_meta(), MembershipRole::Member, 0).unwrap();
        assert_eq!(result.space_id, vec![1; 16]);
        assert_eq!(result.account, vec![3; 20]);
        assert_eq!(result.role, MembershipRole::Member as i32);
        assert_eq!(result.member_space_id, member_space_id);
    }

    #[test]
    fn test_convert_space_left() {
        let action = Action {
            from_id: vec![1; 16],
            to_id: vec![2; 16],
            action: actions::SPACE_LEFT.to_vec(),
            topic: vec![0; 32],
            data: vec![],
        };

        let result = convert_space_left(&action, &test_meta(), 0).unwrap();
        assert_eq!(result.member_id, vec![1; 16]);
        assert_eq!(result.space_id, vec![2; 16]);
    }

    #[test]
    fn test_transform_filters_actions() {
        // ZC16 format: topic contains member's space ID, data contains ABI-encoded address
        let editor_member_space_id: Vec<u8> = vec![0xAA; 16];
        let member_member_space_id: Vec<u8> = vec![0xBB; 16];
        let actions = vec![
            Action {
                from_id: vec![1; 16],
                to_id: vec![],
                action: actions::EDITOR_ADDED.to_vec(),
                topic: editor_member_space_id.iter().copied().chain(vec![0; 16]).collect(),
                data: vec![0; 12].into_iter().chain(vec![2; 20]).collect(), // ABI-encoded address
            },
            Action {
                from_id: vec![3; 16],
                to_id: vec![],
                action: actions::MEMBER_ADDED.to_vec(),
                topic: member_member_space_id.iter().copied().chain(vec![0; 16]).collect(),
                data: vec![0; 12].into_iter().chain(vec![4; 20]).collect(), // ABI-encoded address
            },
            Action {
                from_id: vec![5; 16],
                to_id: vec![6; 16],
                action: actions::SPACE_LEFT.to_vec(),
                topic: vec![7; 32],
                data: vec![],
            },
            // Should NOT be included (different action type)
            Action {
                from_id: vec![8; 16],
                to_id: vec![],
                action: actions::SPACE_REGISTERED.to_vec(),
                topic: vec![9; 32],
                data: vec![],
            },
        ];

        let result = transform(&actions, &test_meta()).unwrap();
        assert_eq!(result.roles_granted.len(), 2); // editor + member added
        assert_eq!(result.roles_revoked.len(), 0);
        assert_eq!(result.spaces_left.len(), 1);
        assert_eq!(result.total(), 3);

        // Verify member_space_id is extracted correctly
        assert_eq!(result.roles_granted[0].member_space_id, editor_member_space_id);
        assert_eq!(result.roles_granted[1].member_space_id, member_member_space_id);
    }
}
