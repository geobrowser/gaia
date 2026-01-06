use hermes_schema::pb::space::{hermes_create_space::Payload, HermesCreateSpace};
use indexer_utils::checksum_address;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::spaces::{SpaceItem, SpaceType};

/// Process a HermesCreateSpace message and return the space item
pub fn handle_create_space(space: &HermesCreateSpace) -> Result<SpaceItem, HandlerError> {
    let space_id = Uuid::from_slice(&space.space_id)?;

    let space_item = match &space.payload {
        Some(Payload::EoaSpace(eoa)) => {
            let owner_address = hex::encode(&eoa.owner);
            let checksummed = checksum_address(format!("0x{}", owner_address));

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
            let space_id_hex = format!("0x{}", hex::encode(&space.space_id));

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
            return Err(HandlerError::MissingPayload);
        }
    };

    Ok(space_item)
}
