use crate::models::properties::{ValueChangeType, ValuesModel};
use grc20::pb::ipfsv2::{op::Payload, Edit, Entity, Op, UnsetEntityValues, Value};

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_edit(ops: Vec<Op>) -> Edit {
        Edit {
            id: b"test_edit_id".to_vec(),
            name: "Test Edit".to_string(),
            ops,
            authors: vec![b"author_id".to_vec()],
            language: None,
        }
    }

    #[test]
    fn test_map_edit_to_values_update_entity() {
        // Create an update entity operation
        let entity = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "value1".to_string(),
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(created_op.entity_id, "entity1");
        assert_eq!(created_op.property_id, "prop1");
        assert_eq!(created_op.space_id, "space1");
        assert_eq!(created_op.value, Some("value1".to_string()));
        assert_eq!(created_op.id, "entity1:prop1:space1");
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_unset_entity_values() {
        // Create an unset entity values operation
        let unset = UnsetEntityValues {
            id: b"entity1".to_vec(),
            properties: vec![b"prop1".to_vec()],
        };

        let op = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], "entity1:prop1:space1");
    }

    #[test]
    fn test_map_edit_to_values_multiple_properties() {
        // Create an update entity operation with multiple properties
        let entity = Entity {
            id: b"entity1".to_vec(),
            values: vec![
                Value {
                    property_id: b"prop1".to_vec(),
                    value: "value1".to_string(),
                },
                Value {
                    property_id: b"prop2".to_vec(),
                    value: "value2".to_string(),
                },
            ],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Sort by property_id to make test deterministic
        let mut created = created;
        created.sort_by(|a, b| a.property_id.cmp(&b.property_id));

        assert_eq!(created[0].property_id, "prop1");
        assert_eq!(created[0].value, Some("value1".to_string()));
        assert_eq!(created[1].property_id, "prop2");
        assert_eq!(created[1].value, Some("value2".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_multiple_entities() {
        // Create operations for multiple entities
        let entity1 = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "value1".to_string(),
            }],
        };

        let entity2 = Entity {
            id: b"entity2".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "value2".to_string(),
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity1)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity2)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Sort by entity_id to make test deterministic
        let mut created = created;
        created.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));

        assert_eq!(created[0].entity_id, "entity1");
        assert_eq!(created[0].property_id, "prop1");
        assert_eq!(created[0].value, Some("value1".to_string()));

        assert_eq!(created[1].entity_id, "entity2");
        assert_eq!(created[1].property_id, "prop1");
        assert_eq!(created[1].value, Some("value2".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_squash_operations() {
        // Test the squashing behavior where multiple operations target the same entity property
        // First create a SET operation
        let entity_set = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "initial_value".to_string(),
            }],
        };

        // Then create another SET operation that updates the same property
        let entity_update = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "updated_value".to_string(),
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity_set)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity_update)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // We should only have one SET operation after squashing
        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        // The value should be the one from the last operation
        assert_eq!(created[0].entity_id, "entity1");
        assert_eq!(created[0].property_id, "prop1");
        assert_eq!(created[0].value, Some("updated_value".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_set_then_delete() {
        // Test a SET followed by a DELETE for the same entity/property
        let entity = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "value1".to_string(),
            }],
        };

        let unset = UnsetEntityValues {
            id: b"entity1".to_vec(),
            properties: vec![b"prop1".to_vec()],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let op2 = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // After squashing, we should only have the DELETE operation
        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], "entity1:prop1:space1");
    }

    #[test]
    fn test_map_edit_to_values_delete_then_set() {
        // Test a DELETE followed by a SET for the same entity/property
        let unset = UnsetEntityValues {
            id: b"entity1".to_vec(),
            properties: vec![b"prop1".to_vec()],
        };

        let entity = Entity {
            id: b"entity1".to_vec(),
            values: vec![Value {
                property_id: b"prop1".to_vec(),
                value: "value1".to_string(),
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // After squashing, we should only have the SET operation
        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        assert_eq!(created[0].entity_id, "entity1");
        assert_eq!(created[0].property_id, "prop1");
        assert_eq!(created[0].value, Some("value1".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_mixed_operations() {
        // Test a mix of operations for different entities and properties
        let entity1 = Entity {
            id: b"entity1".to_vec(),
            values: vec![
                Value {
                    property_id: b"prop1".to_vec(),
                    value: "value1".to_string(),
                },
                Value {
                    property_id: b"prop2".to_vec(),
                    value: "value2".to_string(),
                },
            ],
        };

        let unset1 = UnsetEntityValues {
            id: b"entity1".to_vec(),
            properties: vec![b"prop1".to_vec()],
        };

        let entity2 = Entity {
            id: b"entity2".to_vec(),
            values: vec![Value {
                property_id: b"prop3".to_vec(),
                value: "value3".to_string(),
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity1)),
        };

        let op2 = Op {
            payload: Some(Payload::UnsetEntityValues(unset1)),
        };

        let op3 = Op {
            payload: Some(Payload::UpdateEntity(entity2)),
        };

        let edit = create_test_edit(vec![op1, op2, op3]);
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // After squashing:
        // - entity1:prop1 should be deleted
        // - entity1:prop2 should be set
        // - entity2:prop3 should be set
        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 1);

        // Sort created by entity_id and property_id to make test deterministic
        let mut created = created;
        created.sort_by(|a, b| {
            if a.entity_id != b.entity_id {
                a.entity_id.cmp(&b.entity_id)
            } else {
                a.property_id.cmp(&b.property_id)
            }
        });

        assert_eq!(created[0].entity_id, "entity1");
        assert_eq!(created[0].property_id, "prop2");
        assert_eq!(created[0].value, Some("value2".to_string()));

        assert_eq!(created[1].entity_id, "entity2");
        assert_eq!(created[1].property_id, "prop3");
        assert_eq!(created[1].value, Some("value3".to_string()));

        assert_eq!(deleted[0], "entity1:prop1:space1");
    }
}
