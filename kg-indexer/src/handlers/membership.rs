use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use indexer_utils::checksum_address;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::membership::{EditorItem, MemberItem};

/// Result of processing a membership change
pub enum MembershipChange {
    AddEditor(EditorItem),
    AddMember(MemberItem),
    RemoveEditor(EditorItem),
    RemoveMember(MemberItem),
}

/// Process a HermesRoleGranted message and return the membership change
pub fn handle_role_granted(event: &HermesRoleGranted) -> Result<MembershipChange, IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
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
        Err(_) => Err(IndexerError::parse(format!("Unknown role: {}", event.role))),
    }
}

/// Process a HermesRoleRevoked message and return the membership change
pub fn handle_role_revoked(event: &HermesRoleRevoked) -> Result<MembershipChange, IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
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
        Err(_) => Err(IndexerError::parse(format!("Unknown role: {}", event.role))),
    }
}

fn bytes_to_uuid(bytes: &[u8]) -> Result<Uuid, IndexerError> {
    if bytes.len() != 16 {
        return Err(IndexerError::parse(format!(
            "Invalid UUID bytes length: expected 16, got {}",
            bytes.len()
        )));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Ok(Uuid::from_bytes(arr))
}
