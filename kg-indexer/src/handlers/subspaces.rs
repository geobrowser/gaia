use hermes_schema::pb::space::{HermesSpaceTrustExtension, hermes_space_trust_extension::Extension};
use tracing::debug;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::subspaces::SubspaceItem;
use crate::storage::Storage;

/// Process a HermesSpaceTrustExtension message and write to storage
///
/// Trust extensions represent parent-child relationships between spaces.
/// The "Verified" extension type maps to subspace relationships.
pub async fn handle_trust_extension(
    event: &HermesSpaceTrustExtension,
    storage: &Storage,
) -> Result<(), IndexerError> {
    let parent_space_id = bytes_to_uuid(&event.source_space_id)?;

    match &event.extension {
        Some(Extension::Verified(verified)) => {
            let subspace_id = bytes_to_uuid(&verified.target_space_id)?;

            let subspace = SubspaceItem {
                subspace_id,
                parent_space_id,
            };

            let mut tx = storage.pool.begin().await?;
            storage.insert_subspaces(&[subspace], &mut tx).await?;
            tx.commit().await?;

            debug!(
                parent_space_id = %parent_space_id,
                subspace_id = %subspace_id,
                "Added verified subspace"
            );
        }
        Some(Extension::Related(related)) => {
            // Related spaces are a different relationship type
            // For now we don't store these in the subspaces table
            let target_id = bytes_to_uuid(&related.target_space_id)?;
            debug!(
                source_space_id = %parent_space_id,
                target_space_id = %target_id,
                "Received related extension (not stored as subspace)"
            );
        }
        Some(Extension::Subtopic(subtopic)) => {
            // Subtopic relationships are for topic hierarchies
            let target_id = bytes_to_uuid(&subtopic.target_topic_id)?;
            debug!(
                source_space_id = %parent_space_id,
                target_topic_id = %target_id,
                "Received subtopic extension (not stored as subspace)"
            );
        }
        None => {
            return Err(IndexerError::parse("HermesSpaceTrustExtension has no extension"));
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
