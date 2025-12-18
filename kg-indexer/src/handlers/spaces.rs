use hermes_schema::pb::space::{HermesCreateSpace, hermes_create_space::Payload};
use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};
use tracing::debug;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::spaces::{SpaceItem, SpaceType};
use crate::storage::Storage;

/// Process a HermesCreateSpace message and write to storage
pub async fn handle_create_space(
    space: &HermesCreateSpace,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let space_item = match &space.payload {
        Some(Payload::PersonalSpace(personal)) => {
            // Personal space - derive ID from owner address
            let owner_address = hex::encode(&personal.owner);
            let checksummed = checksum_address(format!("0x{}", owner_address));
            let space_id = derive_space_id(GEO, &checksummed);

            SpaceItem {
                id: space_id,
                space_type: SpaceType::Personal,
                dao_address: checksummed.clone(),
                space_address: checksummed.clone(),
                voting_address: None,
                membership_address: None,
                personal_address: Some(checksummed),
            }
        }
        Some(Payload::DefaultDaoSpace(_dao)) => {
            // Public DAO space - use space_id from message
            let space_id = bytes_to_uuid(&space.space_id)?;

            // For DAO spaces, we need the DAO address which isn't directly in the message
            // We'll derive what we can from the space_id
            let space_id_hex = hex::encode(&space.space_id);

            SpaceItem {
                id: space_id,
                space_type: SpaceType::Public,
                dao_address: space_id_hex.clone(),
                space_address: space_id_hex,
                voting_address: None,
                membership_address: None,
                personal_address: None,
            }
        }
        None => {
            return Err(IndexerError::parse("HermesCreateSpace has no payload"));
        }
    };

    let mut tx = storage.pool.begin().await?;
    storage.insert_spaces(&[space_item.clone()], &mut tx).await?;
    tx.commit().await?;

    debug!(space_id = %space_item.id, space_type = ?space_item.space_type, "Created space");

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
