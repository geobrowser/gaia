use hermes_schema::pb::space::{
    hermes_space_trust_extension::Extension, HermesSpaceTrustExtension,
};
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
    let parent_space_id = Uuid::from_slice(&event.source_space_id)?;

    match &event.extension {
        Some(Extension::Verified(verified)) => {
            let subspace_id = Uuid::from_slice(&verified.target_space_id)?;

            Ok(Some(SubspaceItem {
                subspace_id,
                parent_space_id,
            }))
        }
        Some(Extension::Related(related)) => {
            // Related spaces are a different relationship type
            // For now we don't store these in the subspaces table
            let target_id = Uuid::from_slice(&related.target_space_id)?;
            debug!(
                source_space_id = %parent_space_id,
                target_space_id = %target_id,
                "Received related extension (not stored as subspace)"
            );
            Ok(None)
        }
        Some(Extension::Subtopic(subtopic)) => {
            // Subtopic relationships are for topic hierarchies
            let target_id = Uuid::from_slice(&subtopic.target_topic_id)?;
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
