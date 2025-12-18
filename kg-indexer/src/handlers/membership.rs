use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use indexer_utils::checksum_address;
use tracing::debug;
use uuid::Uuid;

use crate::batch::Batch;
use crate::error::IndexerError;
use crate::models::membership::{EditorItem, MemberItem};

/// Process a HermesRoleGranted message and accumulate into batch
pub fn handle_role_granted(event: &HermesRoleGranted, batch: &mut Batch) -> Result<(), IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => {
            let editor = EditorItem {
                address: account_address.clone(),
                space_id,
            };
            batch.add_editors.push(editor);
            debug!(space_id = %space_id, address = %account_address, "Accumulated editor into batch");
        }
        Ok(MembershipRole::Member) => {
            let member = MemberItem {
                address: account_address.clone(),
                space_id,
            };
            batch.add_members.push(member);
            debug!(space_id = %space_id, address = %account_address, "Accumulated member into batch");
        }
        Err(_) => {
            return Err(IndexerError::parse(format!("Unknown role: {}", event.role)));
        }
    }

    Ok(())
}

/// Process a HermesRoleRevoked message and accumulate into batch
pub fn handle_role_revoked(event: &HermesRoleRevoked, batch: &mut Batch) -> Result<(), IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => {
            let editor = EditorItem {
                address: account_address.clone(),
                space_id,
            };
            batch.remove_editors.push(editor);
            debug!(space_id = %space_id, address = %account_address, "Accumulated editor removal into batch");
        }
        Ok(MembershipRole::Member) => {
            let member = MemberItem {
                address: account_address.clone(),
                space_id,
            };
            batch.remove_members.push(member);
            debug!(space_id = %space_id, address = %account_address, "Accumulated member removal into batch");
        }
        Err(_) => {
            return Err(IndexerError::parse(format!("Unknown role: {}", event.role)));
        }
    }

    Ok(())
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
