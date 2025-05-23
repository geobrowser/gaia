use std::vec;

use grc20::pb::ipfsv2::{Edit, Op, Relation, RelationUpdate, UnsetRelationFields};
use grc20::pb::ipfsv2::op::Payload;

use super::relations::{RelationsModel, SetRelationItem, UpdateRelationItem};

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a byte vector from a string
    fn bytes(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    // Helper function to create an Edit with a single CreateRelation operation
    fn create_edit_with_create_relation() -> Edit {
        let relation = Relation {
            id: bytes("rel1"),
            entity: bytes("entity1"),
            r#type: bytes("type1"),
            from_entity: bytes("from1"),
            to_entity: bytes("to1"),
            to_space: Some(bytes("space1")),
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
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        }
    }

    // Helper function to create an Edit with a single DeleteRelation operation
    fn create_edit_with_delete_relation() -> Edit {
        let op = Op {
            payload: Some(Payload::DeleteRelation(bytes("rel1"))),
        };

        Edit {
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        }
    }

    // Helper function to create an Edit with a single UpdateRelation operation
    fn create_edit_with_update_relation() -> Edit {
        let relation_update = RelationUpdate {
            id: bytes("rel1"),
            from_space: Some(bytes("new_from_space")),
            to_space: Some(bytes("new_to_space")),
            position: Some("new_pos".to_string()),
            verified: Some(false),
            from_version: None,
            to_version: None,
        };

        let op = Op {
            payload: Some(Payload::UpdateRelation(relation_update)),
        };

        Edit {
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        }
    }
    
    // Helper function to create an Edit with a single UnsetRelationFields operation
    fn create_edit_with_unset_relation() -> Edit {
        let unset_relation = UnsetRelationFields {
            id: bytes("rel1"),
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
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![op],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        }
    }

    // Helper function to verify the content of a SetRelationItem
    fn verify_set_relation(item: &SetRelationItem, space_id: &str) {
        assert_eq!(item.id, "rel1");
        assert_eq!(item.entity_id, "entity1");
        assert_eq!(item.type_id, "type1");
        assert_eq!(item.from_id, "from1");
        assert_eq!(item.to_id, "to1");
        assert_eq!(item.to_space_id, Some("space1".to_string()));
        assert_eq!(item.position, Some("pos1".to_string()));
        assert_eq!(item.verified, Some(true));
        assert_eq!(item.space_id, space_id);
    }

    // Helper function to verify the content of an UpdateRelationItem
    fn verify_update_relation(item: &UpdateRelationItem, space_id: &str) {
        assert_eq!(item.id, "rel1");
        assert_eq!(item.from_space_id, Some("new_from_space".to_string()));
        assert_eq!(item.to_space_id, Some("new_to_space".to_string()));
        assert_eq!(item.position, Some("new_pos".to_string()));
        assert_eq!(item.verified, Some(false));
        assert_eq!(item.space_id, space_id);
    }

    #[test]
    fn test_map_edit_to_relations_create() {
        let edit = create_edit_with_create_relation();
        let space_id = "test_space".to_string();

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
        let space_id = "test_space".to_string();

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
        let space_id = "test_space".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], "rel1");
    }
    
    #[test]
    fn test_map_edit_to_relations_unset() {
        let edit = create_edit_with_unset_relation();
        let space_id = "test_space".to_string();

        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 1);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the unset relation
        assert_eq!(unset_relations[0].id, "rel1");
        assert_eq!(unset_relations[0].from_space_id, Some(true));
        assert_eq!(unset_relations[0].to_space_id, Some(true));
        assert_eq!(unset_relations[0].position, Some(true));
        assert_eq!(unset_relations[0].verified, None);
    }

    #[test]
    fn test_squash_create_then_create() {
        // Test create->create scenario: second create overwrites first
        let mut edit = create_edit_with_create_relation();
        
        // Add another create operation with the same ID but different values
        let second_relation = Relation {
            id: bytes("rel1"),
            entity: bytes("entity2"),
            r#type: bytes("type2"),
            from_entity: bytes("from2"),
            to_entity: bytes("to2"),
            to_space: Some(bytes("space2")),
            position: Some("pos2".to_string()),
            verified: Some(false),
            from_space: None,
            from_version: None,
            to_version: None,
        };

        let second_op = Op {
            payload: Some(Payload::CreateRelation(second_relation)),
        };

        edit.ops.push(second_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should be only one set relation (the second one)
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify the second create operation overwrote the first
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "entity2");
        assert_eq!(set_relations[0].type_id, "type2");
        assert_eq!(set_relations[0].from_id, "from2");
        assert_eq!(set_relations[0].to_id, "to2");
        assert_eq!(set_relations[0].to_space_id, Some("space2".to_string()));
        assert_eq!(set_relations[0].position, Some("pos2".to_string()));
        assert_eq!(set_relations[0].verified, Some(false));
    }

    #[test]
    fn test_squash_create_then_update() {
        // Test create->update scenario: updates merge into the create
        let mut edit = create_edit_with_create_relation();
        
        // Add an update operation for the same relation
        let update_relation = RelationUpdate {
            id: bytes("rel1"),
            from_space: Some(bytes("updated_from_space")),
            position: Some("updated_pos".to_string()),
            verified: None, // Leave some fields unset
            to_space: None,
            from_version: None,
            to_version: None,
        };

        let update_op = Op {
            payload: Some(Payload::UpdateRelation(update_relation)),
        };

        edit.ops.push(update_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in one set relation with merged values
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify original values preserved but updated values overrode originals
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "entity1");
        assert_eq!(set_relations[0].type_id, "type1");
        assert_eq!(set_relations[0].from_id, "from1");
        assert_eq!(set_relations[0].to_id, "to1");
        assert_eq!(set_relations[0].from_space_id, Some("updated_from_space".to_string()));
        assert_eq!(set_relations[0].to_space_id, Some("space1".to_string())); // Unchanged
        assert_eq!(set_relations[0].position, Some("updated_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(true)); // Unchanged
    }

    #[test]
    fn test_squash_create_then_delete() {
        // Test create->delete scenario: delete should win
        let mut edit = create_edit_with_create_relation();
        
        // Add a delete operation for the same relation
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes("rel1"))),
        };

        edit.ops.push(delete_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in just a delete
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], "rel1");
    }
    
    #[test]
    fn test_squash_create_then_unset() {
        // Test create->unset scenario: create with fields unset
        let mut edit = create_edit_with_create_relation();
        
        // Add an unset operation for the same relation
        let unset_op = Op {
            payload: Some(Payload::UnsetRelationFields(UnsetRelationFields {
                id: bytes("rel1"),
                position: Some(true), // Unset position
                to_space: Some(true), // Unset to_space
                verified: Some(true), // Unset verified
                from_space: None,     // Don't touch from_space
                from_version: None,   // Don't touch from_version
                to_version: None,     // Don't touch to_version
            })),
        };

        edit.ops.push(unset_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in a create with specified fields unset
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the create relation has the fields unset
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "entity1");
        assert_eq!(set_relations[0].type_id, "type1");
        assert_eq!(set_relations[0].from_id, "from1");
        assert_eq!(set_relations[0].to_id, "to1");
        assert_eq!(set_relations[0].to_space_id, None); // Unset
        assert_eq!(set_relations[0].position, None);    // Unset
        assert_eq!(set_relations[0].verified, None);    // Unset
    }

    #[test]
    fn test_squash_update_then_update() {
        // Test update->update scenario: second update merges into first
        let mut edit = create_edit_with_update_relation();
        
        // Add another update operation with different fields
        let second_update = RelationUpdate {
            id: bytes("rel1"),
            position: Some("newer_pos".to_string()),
            from_space: None, // Don't touch from_space
            to_space: None, // Don't touch to_space
            verified: Some(true), // Override verified
            from_version: Some(bytes("new_from_version")),
            to_version: None,
        };

        let second_op = Op {
            payload: Some(Payload::UpdateRelation(second_update)),
        };

        edit.ops.push(second_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in one update relation with merged values
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);

        // Verify merged update values
        assert_eq!(update_relations[0].id, "rel1");
        assert_eq!(update_relations[0].from_space_id, Some("new_from_space".to_string())); // Unchanged
        assert_eq!(update_relations[0].to_space_id, Some("new_to_space".to_string())); // Unchanged
        assert_eq!(update_relations[0].position, Some("newer_pos".to_string())); // Updated
        
        // We fixed the implementation to properly handle from_version_id
        assert_eq!(update_relations[0].from_version_id, Some("new_from_version".to_string()));
        assert_eq!(update_relations[0].verified, Some(true)); // Updated
    }

    #[test]
    fn test_squash_update_then_delete() {
        // Test update->delete scenario: delete should win
        let mut edit = create_edit_with_update_relation();
        
        // Add a delete operation
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes("rel1"))),
        };

        edit.ops.push(delete_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in just a delete
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], "rel1");
    }
    
    #[test]
    fn test_squash_update_then_unset() {
        // Test update->unset scenario: update with fields unset
        let mut edit = create_edit_with_update_relation();
        
        // Add an unset operation for the same relation
        let unset_op = Op {
            payload: Some(Payload::UnsetRelationFields(UnsetRelationFields {
                id: bytes("rel1"),
                from_space: Some(true), // Unset from_space
                position: Some(true),   // Unset position
                verified: None,         // Don't touch verified
                to_space: None,         // Don't touch to_space
                from_version: None,     // Don't touch from_version
                to_version: None,       // Don't touch to_version
            })),
        };

        edit.ops.push(unset_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in an update with specified fields unset
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the update relation has the fields unset
        assert_eq!(update_relations[0].id, "rel1");
        assert_eq!(update_relations[0].from_space_id, None);   // Unset
        assert_eq!(update_relations[0].position, None);        // Unset
        assert_eq!(update_relations[0].to_space_id, Some("new_to_space".to_string())); // Unchanged
        assert_eq!(update_relations[0].verified, Some(false)); // Unchanged
    }

    #[test]
    fn test_squash_delete_then_create() {
        // Test delete->create scenario: create should win
        let mut edit = create_edit_with_delete_relation();
        
        // Add a create operation after the delete
        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("rel1"),
                entity: bytes("new_entity"),
                r#type: bytes("new_type"),
                from_entity: bytes("new_from"),
                to_entity: bytes("new_to"),
                to_space: Some(bytes("new_space")),
                position: Some("new_pos".to_string()),
                verified: Some(true),
                from_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        edit.ops.push(create_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in just a create
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the create values
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "new_entity");
        assert_eq!(set_relations[0].type_id, "new_type");
    }
    
    #[test]
    fn test_squash_unset_then_update() {
        // Test unset->update scenario: update should overwrite unset fields
        let mut edit = create_edit_with_unset_relation();
        
        // Add an update operation after the unset
        let update_op = Op {
            payload: Some(Payload::UpdateRelation(RelationUpdate {
                id: bytes("rel1"),
                position: Some("new_pos".to_string()),  // Set position
                verified: Some(true),                   // Set verified
                from_space: None,                       // Don't touch from_space
                to_space: None,                         // Don't touch to_space
                from_version: None,
                to_version: None,
            })),
        };

        edit.ops.push(update_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in an update with values from the unset operation
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 1);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the update has values from the unset operation and the update operation
        assert_eq!(update_relations[0].id, "rel1");
        assert_eq!(update_relations[0].from_space_id, None);        // Unset by previous operation
        assert_eq!(update_relations[0].to_space_id, None);          // Unset by previous operation
        assert_eq!(update_relations[0].position, Some("new_pos".to_string())); // Set by update
        assert_eq!(update_relations[0].verified, Some(true));       // Set by update
    }
    
    #[test]
    fn test_squash_unset_then_delete() {
        // Test unset->delete scenario: delete should win
        let mut edit = create_edit_with_unset_relation();
        
        // Add a delete operation
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes("rel1"))),
        };

        edit.ops.push(delete_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in just a delete
        assert_eq!(set_relations.len(), 0);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        assert_eq!(deleted_relations[0], "rel1");
    }
    
    #[test]
    fn test_squash_unset_then_create() {
        // Test unset->create scenario: create should win
        let mut edit = create_edit_with_unset_relation();
        
        // Add a create operation
        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("rel1"),
                entity: bytes("new_entity"),
                r#type: bytes("new_type"),
                from_entity: bytes("new_from"),
                to_entity: bytes("new_to"),
                to_space: Some(bytes("new_space")),
                position: Some("new_pos".to_string()),
                verified: Some(true),
                from_space: None,
                from_version: None,
                to_version: None,
            })),
        };

        edit.ops.push(create_op);
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should result in just a create
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the create values
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "new_entity");
        assert_eq!(set_relations[0].type_id, "new_type");
        assert_eq!(set_relations[0].position, Some("new_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(true));
    }

    #[test]
    fn test_squash_multiple_operations_same_id() {
        // Test a complex sequence: create -> update -> delete -> create
        let mut edit = Edit {
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        };
        
        // 1. Create
        let create_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("rel1"),
                entity: bytes("entity1"),
                r#type: bytes("type1"),
                from_entity: bytes("from1"),
                to_entity: bytes("to1"),
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
                id: bytes("rel1"),
                position: Some("pos2".to_string()),
                verified: Some(false),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };
        
        // 3. Delete
        let delete_op = Op {
            payload: Some(Payload::DeleteRelation(bytes("rel1"))),
        };
        
        // 4. Create again
        let create_again_op = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("rel1"),
                entity: bytes("entity2"),
                r#type: bytes("type2"),
                from_entity: bytes("from2"),
                to_entity: bytes("to2"),
                position: Some("pos3".to_string()),
                verified: Some(true),
                from_space: None,
                to_space: None,
                from_version: None,
                to_version: None,
            })),
        };
        
        edit.ops = vec![create_op, update_op, delete_op, create_again_op];
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Final result should be a create with the last values
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 0);
        
        // Verify the final state matches the last create
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "entity2");
        assert_eq!(set_relations[0].type_id, "type2");
        assert_eq!(set_relations[0].from_id, "from2");
        assert_eq!(set_relations[0].to_id, "to2");
        assert_eq!(set_relations[0].position, Some("pos3".to_string()));
    }

    #[test]
    fn test_multiple_relations_with_different_ids() {
        // Test handling multiple relations with different IDs
        let mut edit = Edit {
            id: bytes("edit_id"),
            name: "test edit".to_string(),
            ops: vec![],
            authors: vec![bytes("author1")],
            language: Some(bytes("en")),
        };
        
        // Create relation 1
        let create_op1 = Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: bytes("rel1"),
                entity: bytes("entity1"),
                r#type: bytes("type1"),
                from_entity: bytes("from1"),
                to_entity: bytes("to1"),
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
                id: bytes("rel2"),
                entity: bytes("entity2"),
                r#type: bytes("type2"),
                from_entity: bytes("from2"),
                to_entity: bytes("to2"),
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
                id: bytes("rel1"),
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
            payload: Some(Payload::DeleteRelation(bytes("rel2"))),
        };
        
        edit.ops = vec![create_op1, create_op2, update_op, delete_op];
        
        let space_id = "test_space".to_string();
        let (set_relations, update_relations, unset_relations, deleted_relations) = 
            RelationsModel::map_edit_to_relations(&edit, &space_id);

        // Should have one create (rel1) and one delete (rel2)
        assert_eq!(set_relations.len(), 1);
        assert_eq!(update_relations.len(), 0);
        assert_eq!(unset_relations.len(), 0);
        assert_eq!(deleted_relations.len(), 1);
        
        // Verify the create for rel1 with updated position
        assert_eq!(set_relations[0].id, "rel1");
        assert_eq!(set_relations[0].entity_id, "entity1");
        assert_eq!(set_relations[0].position, Some("updated_pos".to_string()));
        assert_eq!(set_relations[0].verified, Some(true)); // Unchanged
        
        // Verify the delete for rel2
        assert_eq!(deleted_relations[0], "rel2");
    }
}