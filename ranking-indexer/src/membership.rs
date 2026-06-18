//! Membership event handling.
//!
//! The indexer maintains its own minimal view of the space registry
//! (`ranks.members` / `ranks.editors`), fed from the `space.membership` topic,
//! so eligibility never races the kg-indexer's consumer group. Each applied
//! event recomputes the blocks where the change can matter: blocks in the
//! affected space that hold a submission from the member's personal space.

use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::BlockMeta;
use crate::recompute::recompute_block;
use crate::storage::Storage;

/// A decoded `space.membership` message (dispatched on the `event-type` header).
#[derive(Debug)]
pub enum MembershipEvent {
    RoleGranted(HermesRoleGranted),
    RoleRevoked(HermesRoleRevoked),
}

/// What an event does to the membership view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    AddMember,
    AddEditor,
    RemoveMember,
    RemoveEditor,
}

/// A membership event reduced to ids + effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MembershipChange {
    pub space_id: Uuid,
    pub member_space_id: Uuid,
    pub kind: ChangeKind,
}

/// Block provenance of the event, recorded on entities a triggered recompute
/// mints. Defaults to zero when meta is absent, matching the edit path.
fn block_meta(event: &MembershipEvent) -> BlockMeta {
    let meta = match event {
        MembershipEvent::RoleGranted(e) => e.meta.as_ref(),
        MembershipEvent::RoleRevoked(e) => e.meta.as_ref(),
    };
    meta.map(|m| BlockMeta {
        number: m.block_number as i64,
        timestamp: m.created_at as i64,
    })
    .unwrap_or_default()
}

fn ids(space_id: &[u8], member_space_id: &[u8]) -> Result<(Uuid, Uuid), IndexerError> {
    let space =
        Uuid::from_slice(space_id).map_err(|e| IndexerError::decode(format!("space_id: {e}")))?;
    let member = Uuid::from_slice(member_space_id)
        .map_err(|e| IndexerError::decode(format!("member_space_id: {e}")))?;
    Ok((space, member))
}

/// Reduce a decoded event to the change it makes to the view.
pub fn change_for(event: &MembershipEvent) -> Result<MembershipChange, IndexerError> {
    match event {
        MembershipEvent::RoleGranted(e) => {
            let (space_id, member_space_id) = ids(&e.space_id, &e.member_space_id)?;
            let kind = match MembershipRole::try_from(e.role) {
                Ok(MembershipRole::Editor) => ChangeKind::AddEditor,
                Ok(MembershipRole::Member) => ChangeKind::AddMember,
                Err(_) => return Err(IndexerError::decode(format!("unknown role: {}", e.role))),
            };
            Ok(MembershipChange {
                space_id,
                member_space_id,
                kind,
            })
        }
        MembershipEvent::RoleRevoked(e) => {
            let (space_id, member_space_id) = ids(&e.space_id, &e.member_space_id)?;
            let kind = match MembershipRole::try_from(e.role) {
                Ok(MembershipRole::Editor) => ChangeKind::RemoveEditor,
                Ok(MembershipRole::Member) => ChangeKind::RemoveMember,
                Err(_) => return Err(IndexerError::decode(format!("unknown role: {}", e.role))),
            };
            Ok(MembershipChange {
                space_id,
                member_space_id,
                kind,
            })
        }
    }
}

/// Apply a membership event end to end: update the view, then recompute every
/// block in the affected space holding a submission from that member's
/// personal space. The recompute reads the just-updated view, so a late join
/// integrates the member's existing ranks and a removal drops them.
pub async fn apply_membership_event(
    event: &MembershipEvent,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let change = change_for(event)?;
    let meta = block_meta(event);

    match change.kind {
        ChangeKind::AddMember => {
            storage
                .add_member(change.space_id, change.member_space_id)
                .await?
        }
        ChangeKind::AddEditor => {
            storage
                .add_editor(change.space_id, change.member_space_id)
                .await?
        }
        ChangeKind::RemoveMember => {
            storage
                .remove_member(change.space_id, change.member_space_id)
                .await?
        }
        ChangeKind::RemoveEditor => {
            storage
                .remove_editor(change.space_id, change.member_space_id)
                .await?
        }
    }

    let blocks = storage
        .blocks_with_rankings_from(change.space_id, change.member_space_id)
        .await?;
    for block_id in &blocks {
        recompute_block(*block_id, meta, storage).await?;
    }

    tracing::debug!(
        space_id = %change.space_id,
        member_space_id = %change.member_space_id,
        kind = ?change.kind,
        recomputed_blocks = blocks.len(),
        "ranking-indexer membership: view updated + affected blocks recomputed"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u128) -> Vec<u8> {
        Uuid::from_u128(n).as_bytes().to_vec()
    }

    #[test]
    fn role_granted_maps_to_add() {
        for (role, kind) in [
            (MembershipRole::Member, ChangeKind::AddMember),
            (MembershipRole::Editor, ChangeKind::AddEditor),
        ] {
            let event = MembershipEvent::RoleGranted(HermesRoleGranted {
                space_id: b(1),
                member_space_id: b(2),
                role: role as i32,
                meta: None,
            });
            let change = change_for(&event).unwrap();
            assert_eq!(change.space_id, Uuid::from_u128(1));
            assert_eq!(change.member_space_id, Uuid::from_u128(2));
            assert_eq!(change.kind, kind);
        }
    }

    #[test]
    fn role_revoked_maps_to_remove() {
        for (role, kind) in [
            (MembershipRole::Member, ChangeKind::RemoveMember),
            (MembershipRole::Editor, ChangeKind::RemoveEditor),
        ] {
            let event = MembershipEvent::RoleRevoked(HermesRoleRevoked {
                space_id: b(1),
                member_space_id: b(2),
                role: role as i32,
                meta: None,
            });
            assert_eq!(change_for(&event).unwrap().kind, kind);
        }
    }

    #[test]
    fn unknown_role_is_a_decode_error() {
        let event = MembershipEvent::RoleGranted(HermesRoleGranted {
            space_id: b(1),
            member_space_id: b(2),
            role: 99,
            meta: None,
        });
        assert!(matches!(change_for(&event), Err(IndexerError::Decode(_))));
    }

    #[test]
    fn malformed_ids_are_decode_errors() {
        let event = MembershipEvent::RoleGranted(HermesRoleGranted {
            space_id: vec![1, 2, 3],
            member_space_id: b(2),
            role: MembershipRole::Member as i32,
            meta: None,
        });
        assert!(matches!(change_for(&event), Err(IndexerError::Decode(_))));
    }
}
