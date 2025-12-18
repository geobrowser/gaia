use hermes_schema::pb::space::{hermes_space_trust_extension::Extension, HermesSpaceTrustExtension};
use tracing::debug;
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::subspaces::SubspaceItem;

/// Process a HermesSpaceTrustExtension message and return a subspace if applicable
///
/// Trust extensions represent parent-child relationships between spaces.
/// Only the "Verified" extension type maps to subspace relationships.
/// Returns None for Related and Subtopic extensions.
pub fn handle_trust_extension(
    event: &HermesSpaceTrustExtension,
) -> Result<Option<SubspaceItem>, HandlerError> {
    let parent_space_id = bytes_to_uuid(&event.source_space_id)?;

    match &event.extension {
        Some(Extension::Verified(verified)) => {
            let subspace_id = bytes_to_uuid(&verified.target_space_id)?;

            Ok(Some(SubspaceItem {
                subspace_id,
                parent_space_id,
            }))
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
            Ok(None)
        }
        Some(Extension::Subtopic(subtopic)) => {
            // Subtopic relationships are for topic hierarchies
            let target_id = bytes_to_uuid(&subtopic.target_topic_id)?;
            debug!(
                source_space_id = %parent_space_id,
                target_topic_id = %target_id,
                "Received subtopic extension (not stored as subspace)"
            );
            Ok(None)
        }
        None => Err(HandlerError::MissingPayload),
    }
}

fn bytes_to_uuid(bytes: &[u8]) -> Result<Uuid, HandlerError> {
    if bytes.len() != 16 {
        return Err(HandlerError::InvalidUuidBytes(bytes.len()));
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(bytes);
    Ok(Uuid::from_bytes(arr))
}
