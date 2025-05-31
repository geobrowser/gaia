use std::vec;
use uuid::Uuid;

use grc20::pb::grc20::op::Payload;
use grc20::pb::grc20::{Edit, Op, Relation, RelationUpdate, UnsetRelationFields};

use super::relations::{RelationsModel, SetRelationItem, UpdateRelationItem};

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a byte vector from a UUID string
    fn bytes(s: &str) -> Vec<u8> {
        Uuid::parse_str(s).unwrap().as_bytes().to_vec()
    }

    // Helper function to create an Edit with a single CreateRelation operation
    fn create_edit_with_create_relation() -> Edit {
        let relation = Relation {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            entity: bytes("23456789-1234-4abc-8def-123456789abc"),
            r#type: bytes("34567890-1234-4abc-8def-123456789abc"),
            from_entity: bytes("45678901-1234-4abc-8def-123456789abc"),
            to_entity: bytes("56789012-1234-4abc-8def-123456789abc"),
            to_space: Some(bytes("67890123-1234-4abc-8def-123456789abc")),
            position: Some("pos1".to_string()),
            verified: Some(true),
            from_space: None,
            from_version: None,
            to_version: None,
        };

        let op = Op {
            payload: Some(Payload::CreateRelation(relation)),
        };

        Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        }
    }

    // Helper function to create an Edit with a single DeleteRelation operation
    fn create_edit_with_delete_relation() -> Edit {
        let op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        }
    }

    // Helper function to create an Edit with a single UpdateRelation operation
    fn create_edit_with_update_relation() -> Edit {
        let relation_update = RelationUpdate {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            from_space: Some(bytes("01234567-1234-4abc-8def-123456789abc")),
            to_space: Some(bytes("12345670-1234-4abc-8def-123456789abc")),
            position: Some("new_pos".to_string()),
            verified: Some(false),
            from_version: None,
            to_version: None,
        };

        let op = Op {
            payload: Some(Payload::UpdateRelation(relation_update)),
        };

        Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        }
    }

    // Helper function to create an Edit with a single UnsetRelationFields operation
    fn create_edit_with_unset_relation() -> Edit {
        let unset_relation = UnsetRelationFields {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            from_space: Some(true),
            to_space: Some(true),
            position: Some(true),
            verified: None,
            from_version: None,
            to_version: None,
        };

        let op = Op {
            payload: Some(Payload::UnsetRelationFields(unset_relation)),
        };

        Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        }
    }

    // Helper function to verify a SetRelationItem
    fn verify_set_relation(item: &SetRelationItem, space_id: &str) {
        assert_eq!(item.id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(item.entity_id, Uuid::parse_str("23456789-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(item.type_id, Uuid::parse_str("34567890-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(item.from_id, Uuid::parse_str("45678901-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(item.to_id, Uuid::parse_str("56789012-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            item.to_space_id,
            Some("67890123-1234-4abc-8def-123456789abc".to_string())
        );
        assert_eq!(item.position, Some("pos1".to_string()));
        assert_eq!(item.verified, Some(true));
        assert_eq!(item.space_id, space_id);
    }

    // Helper function to verify an UpdateRelationItem
    fn verify_update_relation(item: &UpdateRelationItem, space_id: &str) {
        assert_eq!(item.id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            item.from_space_id,
            Some("01234567-1234-4abc-8def-123456789abc".to_string())
        );
        assert_eq!(
            item.to_space_id,
            Some("12345670-1234-4abc-8def-123456789abc".to_string())
        );
        assert_eq!(item.position, Some("new_pos".to_string()));
        assert_eq!(item.verified, Some(false));
        assert_eq!(item.space_id, space_id);
    }

    #[test]
    fn test_map_edit_to_relations_create() {
        let edit = create_edit_with_create_relation();
        let space_id = "space123".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        verify_set_relation(&set_relations[0], &space_id);
    }

    #[test]
    fn test_map_edit_to_relations_update() {
        let edit = create_edit_with_update_relation();
        let space_id = "space123".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        verify_update_relation(&update_relations[0], &space_id);
    }

    #[test]
    fn test_map_edit_to_relations_delete() {
        let edit = create_edit_with_delete_relation();
        let space_id = "space123".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
    }

    #[test]
    fn test_map_edit_to_relations_unset() {
        let edit = create_edit_with_unset_relation();
        let space_id = "space123".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 1);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the unset relation
        assert_eq!(
            unset_relations[0].id,
            Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(unset_relations[0].from_space_id, Some(true));
        assert_eq!(unset_relations[0].to_space_id, Some(true));
        assert_eq!(unset_relations[0].position, Some(true));
        assert_eq!(unset_relations[0].space_id, space_id);
    }

    #[test]
    fn test_squash_create_then_create() {
        let first_relation = create_edit_with_create_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let second_relation = Relation {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            entity: bytes("a1234567-1234-4abc-8def-123456789abc"),
            r#type: bytes("b1234567-1234-4abc-8def-123456789abc"),
            from_entity: bytes("c1234567-1234-4abc-8def-123456789abc"),
            to_entity: bytes("d1234567-1234-4abc-8def-123456789abc"),
            to_space: Some(bytes("e1234567-1234-4abc-8def-123456789abc")),
            position: Some("pos2".to_string()),
            verified: Some(false),
            from_space: None,
            from_version: None,
            to_version: None,
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(first_relation),
                },
                Op {
                    payload: Some(Payload::CreateRelation(second_relation)),
                },
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the second create operation overwrote the first
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("a1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("b1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].from_id,
            Uuid::parse_str("c1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].to_id,
            Uuid::parse_str("d1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].to_space_id,
            Some("e1234567-1234-4abc-8def-123456789abc".to_string())
        );
        assert_eq!(set_relations[0].position, Some("pos2".to_string()));
        assert_eq!(set_relations[0].verified, Some(false));
    }

    #[test]
    fn test_squash_create_then_update() {
        let create_relation = create_edit_with_create_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let update_relation = RelationUpdate {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            from_space: Some(bytes("f1234567-1234-4abc-8def-123456789abc")),
            to_space: Some(bytes("f2345678-1234-4abc-8def-123456789abc")),
            position: Some("updated_pos".to_string()),
            verified: Some(false),
            from_version: None,
            to_version: None,
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(create_relation),
                },
                Op {
                    payload: Some(Payload::UpdateRelation(update_relation)),
                },
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify original values preserved but updated values overrode originals
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("23456789-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("34567890-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].from_id,
            Uuid::parse_str("45678901-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].to_id,
            Uuid::parse_str("56789012-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].from_space_id,
            Some("f1234567-1234-4abc-8def-123456789abc".to_string())
        );
        assert_eq!(
            set_relations[0].to_space_id,
            Some("f2345678-1234-4abc-8def-123456789abc".to_string())
        ); // Updated
        assert_eq!(set_relations[0].position, Some("updated_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(false)); // Updated
    }

    #[test]
    fn test_squash_create_then_delete() {
        let create_relation = create_edit_with_create_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(create_relation),
                },
                delete_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Create then delete should result in no operations
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
    }

    #[test]
    fn test_squash_create_then_unset() {
        let create_relation = create_edit_with_create_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let unset_op = Op {
            payload: Some(Payload::UnsetRelationFields(UnsetRelationFields {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                position: Some(true), // Unset position
                to_space: Some(true), // Unset to_space
                verified: Some(true), // Unset verified
                from_space: None,     // Don't touch from_space
                from_version: None,   // Don't touch from_version
                to_version: None,     // Don't touch to_version
            })),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(create_relation),
                },
                unset_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the create relation has the fields unset
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("23456789-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("34567890-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].from_id,
            Uuid::parse_str("45678901-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].to_id,
            Uuid::parse_str("56789012-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(set_relations[0].to_space_id, None); // Unset
        assert_eq!(set_relations[0].position, None); // Unset
        assert_eq!(set_relations[0].verified, None); // Unset
    }

    #[test]
    fn test_squash_update_then_update() {
        let first_update = create_edit_with_update_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let second_update = RelationUpdate {
            id: bytes("12345678-1234-4abc-8def-123456789abc"),
            position: Some("newer_pos".to_string()),
            from_space: None,     // Don't touch from_space
            to_space: None,       // Don't touch to_space
            verified: Some(true), // Override verified
            from_version: Some(bytes("f1234567-1234-4abc-8def-123456789abc")),
            to_version: None,
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(first_update),
                },
                Op {
                    payload: Some(Payload::UpdateRelation(second_update)),
                },
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the update values
        assert_eq!(
            update_relations[0].id,
            Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            update_relations[0].from_space_id,
            Some("01234567-1234-4abc-8def-123456789abc".to_string())
        ); // Unchanged
        assert_eq!(
            update_relations[0].to_space_id,
            Some("12345670-1234-4abc-8def-123456789abc".to_string())
        ); // Unchanged
        assert_eq!(update_relations[0].position, Some("newer_pos".to_string())); // Updated
        assert_eq!(update_relations[0].verified, Some(true)); // Updated
        assert_eq!(
            update_relations[0].from_version_id,
            Some("f1234567-1234-4abc-8def-123456789abc".to_string())
        );
    }

    #[test]
    fn test_squash_update_then_delete() {
        let update_relation = create_edit_with_update_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(update_relation),
                },
                delete_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Update then delete should result in just delete
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
    }

    #[test]
    fn test_squash_update_then_unset() {
        let update_relation = create_edit_with_update_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let unset_op = Op {
            payload: Some(Payload::UnsetRelationFields(UnsetRelationFields {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                from_space: Some(true), // Unset from_space
                position: Some(true),   // Unset position
                verified: None,         // Don't touch verified
                to_space: None,         // Don't touch to_space
                from_version: None,     // Don't touch from_version
                to_version: None,       // Don't touch to_version
            })),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(update_relation),
                },
                unset_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the update relation has the fields unset
        assert_eq!(
            update_relations[0].id,
            Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(update_relations[0].from_space_id, None); // Unset
        assert_eq!(update_relations[0].position, None); // Unset
        assert_eq!(
            update_relations[0].to_space_id,
            Some("12345670-1234-4abc-8def-123456789abc".to_string())
        ); // Unchanged
        assert_eq!(update_relations[0].verified, Some(false)); // Unchanged
    }

    #[test]
    fn test_squash_delete_then_create() {
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                entity: bytes("a1234567-1234-4abc-8def-123456789abc"),
                r#type: bytes("b1234567-1234-4abc-8def-123456789abc"),
                from_entity: bytes("c1234567-1234-4abc-8def-123456789abc"),
                to_entity: bytes("d1234567-1234-4abc-8def-123456789abc"),
                to_space: Some(bytes("e1234567-1234-4abc-8def-123456789abc")),
                position: Some("new_pos".to_string()),
                verified: Some(true),
                from_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![delete_op, create_op],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the create values
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("a1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("b1234567-1234-4abc-8def-123456789abc").unwrap()
        );
    }

    #[test]
    fn test_squash_unset_then_update() {
        let unset_op = create_edit_with_unset_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let update_op = Op {
            payload: Some(Payload::UpdateRelation(RelationUpdate {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                position: Some("new_pos".to_string()), // Set position
                verified: Some(true),                  // Set verified
                from_space: None,                      // Don't touch from_space
                to_space: None,                        // Don't touch to_space
                from_version: None,
                to_version: None,
            })),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(unset_op),
                },
                update_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the update has values from the unset operation and the update operation
        assert_eq!(
            update_relations[0].id,
            Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(update_relations[0].from_space_id, None); // Unset by previous operation
        assert_eq!(update_relations[0].to_space_id, None); // Unset by previous operation
        assert_eq!(update_relations[0].position, Some("new_pos".to_string())); // Set by update
        assert_eq!(update_relations[0].verified, Some(true)); // Set by update
    }

    #[test]
    fn test_squash_unset_then_delete() {
        let unset_op = create_edit_with_unset_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(unset_op),
                },
                delete_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Unset then delete should result in just delete
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
    }

    #[test]
    fn test_squash_unset_then_create() {
        let unset_op = create_edit_with_unset_relation().ops[0]
            .payload
            .as_ref()
            .unwrap()
            .clone();

        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                entity: bytes("a1234567-1234-4abc-8def-123456789abc"),
                r#type: bytes("b1234567-1234-4abc-8def-123456789abc"),
                from_entity: bytes("c1234567-1234-4abc-8def-123456789abc"),
                to_entity: bytes("d1234567-1234-4abc-8def-123456789abc"),
                to_space: Some(bytes("e1234567-1234-4abc-8def-123456789abc")),
                position: Some("new_pos".to_string()),
                verified: Some(true),
                from_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        let edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![
                Op {
                    payload: Some(unset_op),
                },
                create_op,
            ],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Unset then create should result in just create
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the create values
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("a1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("b1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(set_relations[0].position, Some("new_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(true));
    }

    #[test]
    fn test_squash_multiple_operations_same_id() {
        // Test a complex sequence: create -> update -> delete -> create
        let mut edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        // 1. Create
        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                entity: bytes("23456789-1234-4abc-8def-123456789abc"),
                r#type: bytes("34567890-1234-4abc-8def-123456789abc"),
                from_entity: bytes("45678901-1234-4abc-8def-123456789abc"),
                to_entity: bytes("56789012-1234-4abc-8def-123456789abc"),
                position: Some("pos1".to_string()),
                verified: Some(true),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // 2. Update
        let update_op = Op {
            payload: Some(Payload::UpdateRelation(RelationUpdate {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                position: Some("updated_pos".to_string()),
                verified: Some(false),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // 3. Delete
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "12345678-1234-4abc-8def-123456789abc",
            ))),
        };

        // 4. Create again
        let create_again_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                entity: bytes("a1234567-1234-4abc-8def-123456789abc"),
                r#type: bytes("b1234567-1234-4abc-8def-123456789abc"),
                from_entity: bytes("c1234567-1234-4abc-8def-123456789abc"),
                to_entity: bytes("d1234567-1234-4abc-8def-123456789abc"),
                position: Some("pos3".to_string()),
                verified: Some(true),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        edit.ops = vec![create_op, update_op, delete_op, create_again_op];

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Final result should be a create with the last values
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the final state matches the last create
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("a1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].type_id,
            Uuid::parse_str("b1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].from_id,
            Uuid::parse_str("c1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(
            set_relations[0].to_id,
            Uuid::parse_str("d1234567-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(set_relations[0].position, Some("pos3".to_string()));
        assert_eq!(set_relations[0].verified, Some(true));
    }

    #[test]
    fn test_multiple_relations_with_different_ids() {
        // Test handling multiple relations with different IDs
        let mut edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        // Create relation 1
        let create_op1 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                entity: bytes("23456789-1234-4abc-8def-123456789abc"),
                r#type: bytes("34567890-1234-4abc-8def-123456789abc"),
                from_entity: bytes("45678901-1234-4abc-8def-123456789abc"),
                to_entity: bytes("56789012-1234-4abc-8def-123456789abc"),
                position: Some("pos1".to_string()),
                verified: Some(true),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // Create relation 2
        let create_op2 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("87654321-1234-4abc-8def-123456789abc"),
                entity: bytes("a1234567-1234-4abc-8def-123456789abc"),
                r#type: bytes("b1234567-1234-4abc-8def-123456789abc"),
                from_entity: bytes("c1234567-1234-4abc-8def-123456789abc"),
                to_entity: bytes("d1234567-1234-4abc-8def-123456789abc"),
                position: Some("pos2".to_string()),
                verified: Some(false),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // Update relation 1
        let update_op = Op {
            payload: Some(Payload::UpdateRelation(RelationUpdate {
                id: bytes("12345678-1234-4abc-8def-123456789abc"),
                position: Some("updated_pos".to_string()),
                verified: None,
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // Delete relation 2
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes(
                "87654321-1234-4abc-8def-123456789abc",
            ))),
        };

        edit.ops = vec![create_op1, create_op2, update_op, delete_op];

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should have one create (rel1) and one delete (rel2)
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);

        // Verify the create for rel1 with updated position
        assert_eq!(set_relations[0].id, Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(
            set_relations[0].entity_id,
            Uuid::parse_str("23456789-1234-4abc-8def-123456789abc").unwrap()
        );
        assert_eq!(set_relations[0].position, Some("updated_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(true)); // Unchanged

        // Verify the delete for rel2
        assert_eq!(deleted_relations[0], Uuid::parse_str("87654321-1234-4abc-8def-123456789abc").unwrap());
    }

    #[test]
    fn test_multiple_create_relations_different_entities() {
        // Test creating multiple relations with different entity IDs in a single edit
        let mut edit = Edit {
            id: bytes("78901234-1234-4abc-8def-123456789abc"),
            name: "test edit".to_string(),
            ops: vec![],
            authors: vec![bytes("89012345-1234-4abc-8def-123456789abc")],
            language: Some(bytes("90123456-1234-4abc-8def-123456789abc")),
        };

        // Create relation 1
        let create_op1 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("11111111-1111-4abc-8def-123456789abc"),
                entity: bytes("entity01-1234-4abc-8def-123456789abc"),
                r#type: bytes("type0001-1234-4abc-8def-123456789abc"),
                from_entity: bytes("from0001-1234-4abc-8def-123456789abc"),
                to_entity: bytes("to000001-1234-4abc-8def-123456789abc"),
                position: Some("position1".to_string()),
                verified: Some(true),
                from_space: None,
                to_space: Some(bytes("space001-1234-4abc-8def-123456789abc")),
                from_version: None,
                to_version: None,
            })),
        };

        // Create relation 2
        let create_op2 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("22222222-2222-4abc-8def-123456789abc"),
                entity: bytes("entity02-1234-4abc-8def-123456789abc"),
                r#type: bytes("type0002-1234-4abc-8def-123456789abc"),
                from_entity: bytes("from0002-1234-4abc-8def-123456789abc"),
                to_entity: bytes("to000002-1234-4abc-8def-123456789abc"),
                position: Some("position2".to_string()),
                verified: Some(false),
                from_space: Some(bytes("space002-1234-4abc-8def-123456789abc")),
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        // Create relation 3
        let create_op3 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("33333333-3333-4abc-8def-123456789abc"),
                entity: bytes("entity03-1234-4abc-8def-123456789abc"),
                r#type: bytes("type0003-1234-4abc-8def-123456789abc"),
                from_entity: bytes("from0003-1234-4abc-8def-123456789abc"),
                to_entity: bytes("to000003-1234-4abc-8def-123456789abc"),
                position: None,
                verified: None,
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        edit.ops = vec![create_op1, create_op2, create_op3];

        let space_id = "space123".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) =
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should have three creates and nothing else
        assert_eq!(set_relations.len(), 3);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify relation 1
        let rel1 = &set_relations[0];
        assert_eq!(rel1.id, Uuid::parse_str("11111111-1111-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel1.entity_id, Uuid::parse_str("entity01-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel1.type_id, Uuid::parse_str("type0001-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel1.from_id, Uuid::parse_str("from0001-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel1.to_id, Uuid::parse_str("to000001-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel1.position, Some("position1".to_string()));
        assert_eq!(rel1.verified, Some(true));
        assert_eq!(rel1.space_id, "space123");

        // Verify relation 2
        let rel2 = &set_relations[1];
        assert_eq!(rel2.id, Uuid::parse_str("22222222-2222-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel2.entity_id, Uuid::parse_str("entity02-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel2.type_id, Uuid::parse_str("type0002-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel2.from_id, Uuid::parse_str("from0002-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel2.to_id, Uuid::parse_str("to000002-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel2.position, Some("position2".to_string()));
        assert_eq!(rel2.verified, Some(false));
        assert_eq!(rel2.space_id, "space123");

        // Verify relation 3
        let rel3 = &set_relations[2];
        assert_eq!(rel3.id, Uuid::parse_str("33333333-3333-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel3.entity_id, Uuid::parse_str("entity03-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel3.type_id, Uuid::parse_str("type0003-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel3.from_id, Uuid::parse_str("from0003-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel3.to_id, Uuid::parse_str("to000003-1234-4abc-8def-123456789abc").unwrap());
        assert_eq!(rel3.position, None);
        assert_eq!(rel3.verified, None);
        assert_eq!(rel3.space_id, "space123");
    }
}
