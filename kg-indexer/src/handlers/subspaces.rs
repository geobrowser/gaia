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

#[cfg(test)]
mod tests {
    use super::*;
    use hermes_schema::pb::space::{
        RelatedExtension, RelatedRemoval, SubtopicExtension, SubtopicRemoval, VerifiedExtension,
        VerifiedRemoval,
    };

    fn make_uuid_bytes(last_byte: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 15];
        bytes.push(last_byte);
        bytes
    }

    fn make_event(source: u8, extension: Option<Extension>) -> HermesSpaceTrustExtension {
        HermesSpaceTrustExtension {
            source_space_id: make_uuid_bytes(source),
            extension,
            meta: None,
        }
    }

    #[test]
    fn test_verified_returns_insert_explicit() {
        let event = make_event(
            0x01,
            Some(Extension::Verified(VerifiedExtension {
                target_space_id: make_uuid_bytes(0x02),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::InsertExplicit(item) => {
                assert_eq!(
                    item.parent_space_id,
                    Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
                );
                assert_eq!(
                    item.subspace_id,
                    Uuid::from_slice(&make_uuid_bytes(0x02)).unwrap()
                );
                assert!(matches!(item.subspace_type, SubspaceType::Verified));
            }
            _ => panic!("Expected InsertExplicit, got {:?}", change),
        }
    }

    #[test]
    fn test_related_returns_insert_explicit() {
        let event = make_event(
            0x01,
            Some(Extension::Related(RelatedExtension {
                target_space_id: make_uuid_bytes(0x02),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::InsertExplicit(item) => {
                assert!(matches!(item.subspace_type, SubspaceType::Related));
            }
            _ => panic!("Expected InsertExplicit, got {:?}", change),
        }
    }

    #[test]
    fn test_subtopic_returns_insert_topic() {
        let event = make_event(
            0x01,
            Some(Extension::Subtopic(SubtopicExtension {
                target_topic_id: make_uuid_bytes(0x03),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::InsertTopic(item) => {
                assert_eq!(
                    item.space_id,
                    Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
                );
                assert_eq!(
                    item.topic_id,
                    Uuid::from_slice(&make_uuid_bytes(0x03)).unwrap()
                );
            }
            _ => panic!("Expected InsertTopic, got {:?}", change),
        }
    }

    #[test]
    fn test_verified_removal_returns_remove_explicit() {
        let event = make_event(
            0x01,
            Some(Extension::VerifiedRemoval(VerifiedRemoval {
                target_space_id: make_uuid_bytes(0x02),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::RemoveExplicit(item) => {
                assert_eq!(
                    item.parent_space_id,
                    Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
                );
                assert_eq!(
                    item.subspace_id,
                    Uuid::from_slice(&make_uuid_bytes(0x02)).unwrap()
                );
                assert!(matches!(item.subspace_type, SubspaceType::Verified));
            }
            _ => panic!("Expected RemoveExplicit, got {:?}", change),
        }
    }

    #[test]
    fn test_related_removal_returns_remove_explicit() {
        let event = make_event(
            0x01,
            Some(Extension::RelatedRemoval(RelatedRemoval {
                target_space_id: make_uuid_bytes(0x02),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::RemoveExplicit(item) => {
                assert!(matches!(item.subspace_type, SubspaceType::Related));
            }
            _ => panic!("Expected RemoveExplicit, got {:?}", change),
        }
    }

    #[test]
    fn test_subtopic_removal_returns_remove_topic() {
        let event = make_event(
            0x01,
            Some(Extension::SubtopicRemoval(SubtopicRemoval {
                target_topic_id: make_uuid_bytes(0x03),
            })),
        );

        let change = handle_trust_extension(&event).unwrap().unwrap();
        match change {
            SubspaceChange::RemoveTopic(item) => {
                assert_eq!(
                    item.space_id,
                    Uuid::from_slice(&make_uuid_bytes(0x01)).unwrap()
                );
                assert_eq!(
                    item.topic_id,
                    Uuid::from_slice(&make_uuid_bytes(0x03)).unwrap()
                );
            }
            _ => panic!("Expected RemoveTopic, got {:?}", change),
        }
    }

    #[test]
    fn test_none_extension_returns_ok_none() {
        let event = make_event(0x01, None);

        let result = handle_trust_extension(&event).unwrap();
        assert!(result.is_none(), "None extension should return Ok(None)");
    }

    #[test]
    fn test_invalid_uuid_returns_error() {
        let event = HermesSpaceTrustExtension {
            source_space_id: vec![0u8; 5], // invalid: 5 bytes, not 16
            extension: Some(Extension::Verified(VerifiedExtension {
                target_space_id: make_uuid_bytes(0x02),
            })),
            meta: None,
        };

        assert!(handle_trust_extension(&event).is_err());
    }
}
