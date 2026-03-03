use hermes_schema::pb::space::{
    hermes_space_trust_extension::Extension, HermesSpaceTrustExtension,
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::HandlerError;
use crate::models::subspaces::{SubspaceChange, SubspaceItem, SubspaceTopicItem, SubspaceType};

/// Process a HermesSpaceTrustExtension message and return the corresponding
/// storage operation.
///
/// Maps each proto extension variant to a `SubspaceChange`:
/// - Verified/Related → InsertExplicit/RemoveExplicit (space→space edges)
/// - Subtopic/SubtopicRemoval → InsertTopic/RemoveTopic (space→topic edges)
///
/// Returns `Ok(None)` for unknown extension variants (rolling deploy resilience:
/// an old kg-indexer may receive new proto variants it doesn't know about, which
/// protobuf decodes as `extension = None`).
pub fn handle_trust_extension(
    event: &HermesSpaceTrustExtension,
) -> Result<Option<SubspaceChange>, HandlerError> {
    let source_space_id = Uuid::from_slice(&event.source_space_id)?;

    match &event.extension {
        // Explicit additions (space → space)
        Some(Extension::Verified(v)) => {
            let target = Uuid::from_slice(&v.target_space_id)?;
            debug!(
                parent_space_id = %source_space_id,
                child_space_id = %target,
                subspace_type = "verified",
                "Insert verified subspace"
            );
            Ok(Some(SubspaceChange::InsertExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Verified,
            })))
        }
        Some(Extension::Related(r)) => {
            let target = Uuid::from_slice(&r.target_space_id)?;
            debug!(
                parent_space_id = %source_space_id,
                child_space_id = %target,
                subspace_type = "related",
                "Insert related subspace"
            );
            Ok(Some(SubspaceChange::InsertExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Related,
            })))
        }
        // Topic additions (space → topic)
        Some(Extension::Subtopic(s)) => {
            let topic = Uuid::from_slice(&s.target_topic_id)?;
            debug!(
                space_id = %source_space_id,
                topic_id = %topic,
                "Insert subspace topic"
            );
            Ok(Some(SubspaceChange::InsertTopic(SubspaceTopicItem {
                space_id: source_space_id,
                topic_id: topic,
            })))
        }
        // Explicit removals (space → space)
        Some(Extension::VerifiedRemoval(v)) => {
            let target = Uuid::from_slice(&v.target_space_id)?;
            debug!(
                parent_space_id = %source_space_id,
                child_space_id = %target,
                subspace_type = "verified",
                "Remove verified subspace"
            );
            Ok(Some(SubspaceChange::RemoveExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Verified,
            })))
        }
        Some(Extension::RelatedRemoval(r)) => {
            let target = Uuid::from_slice(&r.target_space_id)?;
            debug!(
                parent_space_id = %source_space_id,
                child_space_id = %target,
                subspace_type = "related",
                "Remove related subspace"
            );
            Ok(Some(SubspaceChange::RemoveExplicit(SubspaceItem {
                subspace_id: target,
                parent_space_id: source_space_id,
                subspace_type: SubspaceType::Related,
            })))
        }
        // Topic removals (space → topic)
        Some(Extension::SubtopicRemoval(s)) => {
            let topic = Uuid::from_slice(&s.target_topic_id)?;
            debug!(
                space_id = %source_space_id,
                topic_id = %topic,
                "Remove subspace topic"
            );
            Ok(Some(SubspaceChange::RemoveTopic(SubspaceTopicItem {
                space_id: source_space_id,
                topic_id: topic,
            })))
        }
        // Unknown variant — likely a newer proto field this binary doesn't know about.
        // Log and skip rather than failing the block transaction.
        None => {
            warn!(
                source_space_id = %source_space_id,
                "Received trust extension with no extension variant (unknown proto field?) — skipping"
            );
            Ok(None)
        }
    }
}
