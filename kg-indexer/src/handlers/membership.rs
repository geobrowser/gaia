use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use indexer_utils::checksum_address;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::membership::{EditorItem, MemberItem};

/// Result of processing a membership change
pub enum MembershipChange {
    AddEditor(EditorItem),
    AddMember(MemberItem),
    RemoveEditor(EditorItem),
    RemoveMember(MemberItem),
}

/// Process a HermesRoleGranted message and return the membership change
pub fn handle_role_granted(event: &HermesRoleGranted) -> Result<MembershipChange, HandlerError> {
    let space_id = Uuid::from_slice(&event.space_id)?;
    if event.account.len() != 20 {
        return Err(HandlerError::InvalidAddress(format!(
            "Expected 20 bytes, got {}",
            event.account.len()
        )));
    }
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => Ok(MembershipChange::AddEditor(EditorItem {
            address: account_address,
            space_id,
        })),
        Ok(MembershipRole::Member) => Ok(MembershipChange::AddMember(MemberItem {
            address: account_address,
            space_id,
        })),
        Err(_) => Err(HandlerError::UnknownRole(event.role)),
    }
}

/// Process a HermesRoleRevoked message and return the membership change
pub fn handle_role_revoked(event: &HermesRoleRevoked) -> Result<MembershipChange, HandlerError> {
    let space_id = Uuid::from_slice(&event.space_id)?;
    if event.account.len() != 20 {
        return Err(HandlerError::InvalidAddress(format!(
            "Expected 20 bytes, got {}",
            event.account.len()
        )));
    }
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => Ok(MembershipChange::RemoveEditor(EditorItem {
            address: account_address,
            space_id,
        })),
        Ok(MembershipRole::Member) => Ok(MembershipChange::RemoveMember(MemberItem {
            address: account_address,
            space_id,
        })),
        Err(_) => Err(HandlerError::UnknownRole(event.role)),
    }
}
