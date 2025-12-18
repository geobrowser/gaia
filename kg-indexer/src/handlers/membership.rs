use hermes_schema::pb::membership::{HermesRoleGranted, HermesRoleRevoked, MembershipRole};
use indexer_utils::checksum_address;
use tracing::debug;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::membership::{EditorItem, MemberItem};
use crate::storage::Storage;

/// Process a HermesRoleGranted message and write to storage
pub async fn handle_role_granted(
    event: &HermesRoleGranted,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    let mut tx = storage.pool.begin().await?;

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => {
            let editor = EditorItem {
                address: account_address.clone(),
                space_id,
            };
            storage.insert_editors(&[editor], &mut tx).await?;
            debug!(space_id = %space_id, address = %account_address, "Added editor");
        }
        Ok(MembershipRole::Member) => {
            let member = MemberItem {
                address: account_address.clone(),
                space_id,
            };
            storage.insert_members(&[member], &mut tx).await?;
            debug!(space_id = %space_id, address = %account_address, "Added member");
        }
        Err(_) => {
            return Err(IndexerError::parse(format!("Unknown role: {}", event.role)));
        }
    }

    tx.commit().await?;
    Ok(())
}

/// Process a HermesRoleRevoked message and write to storage
pub async fn handle_role_revoked(
    event: &HermesRoleRevoked,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let space_id = bytes_to_uuid(&event.space_id)?;
    let account_address = checksum_address(format!("0x{}", hex::encode(&event.account)));

    let mut tx = storage.pool.begin().await?;

    match MembershipRole::try_from(event.role) {
        Ok(MembershipRole::Editor) => {
            let editor = EditorItem {
                address: account_address.clone(),
                space_id,
            };
            storage.remove_editors(&[editor], &mut tx).await?;
            debug!(space_id = %space_id, address = %account_address, "Removed editor");
        }
        Ok(MembershipRole::Member) => {
            let member = MemberItem {
                address: account_address.clone(),
                space_id,
            };
            storage.remove_members(&[member], &mut tx).await?;
            debug!(space_id = %space_id, address = %account_address, "Removed member");
        }
        Err(_) => {
            return Err(IndexerError::parse(format!("Unknown role: {}", event.role)));
        }
    }

    tx.commit().await?;
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
