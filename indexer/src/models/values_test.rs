use crate::models::values::{ValueChangeType, ValuesModel};
use uuid::Uuid;
use wire::pb::grc20::{
    op::Payload, options, Edit, Entity, NumberOptions, Op, Options, TextOptions, UnsetEntityValues,
    Value,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_edit(ops: Vec<Op>) -> Edit {
        Edit {
            id: Uuid::parse_str("f47ac10b-58cc-4372-a567-0e02b2c3d479")
                .unwrap()
                .as_bytes()
                .to_vec(),
            name: "Test Edit".to_string(),
            ops,
            authors: vec![Uuid::parse_str("f47ac10b-58cc-4372-a567-0e02b2c3d480")
                .unwrap()
                .as_bytes()
                .to_vec()],
            language: None,
        }
    }

    #[test]
    fn test_map_edit_to_values_update_entity() {
        // Create an update entity operation
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value1".to_string(),
                options: None,
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(
            created_op.space_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(created_op.value, Some("value1".to_string()));
        // The id is now a deterministically generated UUID, so we just check it exists
        assert_ne!(created_op.id, Uuid::nil());
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_unset_entity_values() {
        // Create an unset entity values operation
        let unset = UnsetEntityValues {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            properties: vec![Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                .unwrap()
                .as_bytes()
                .to_vec()],
        };

        let op = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        // The deleted ID is now a deterministically generated UUID
        assert_ne!(deleted[0], Uuid::nil());
    }

    #[test]
    fn test_map_edit_to_values_multiple_properties() {
        // Create an update entity operation with multiple properties
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "value1".to_string(),
                    options: None,
                },
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "value2".to_string(),
                    options: None,
                },
            ],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Check first value
        let first_op = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
            })
            .unwrap();
        assert_eq!(
            first_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(first_op.value, Some("value1".to_string()));
        assert!(matches!(first_op.change_type, ValueChangeType::SET));

        // Check second value
        let second_op = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap()
            })
            .unwrap();
        assert_eq!(
            second_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(second_op.value, Some("value2".to_string()));
        assert!(matches!(second_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_multiple_entities() {
        // Create update entity operations for multiple entities
        let entity1 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value1".to_string(),
                options: None,
            }],
        };

        let entity2 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value2".to_string(),
                options: None,
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity1)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity2)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Check first entity value
        let first_op = created
            .iter()
            .find(|op| {
                op.entity_id == Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
            })
            .unwrap();
        assert_eq!(
            first_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(first_op.value, Some("value1".to_string()));
        assert!(matches!(first_op.change_type, ValueChangeType::SET));

        // Check second entity value
        let second_op = created
            .iter()
            .find(|op| {
                op.entity_id == Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap()
            })
            .unwrap();
        assert_eq!(
            second_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap()
        );
        assert_eq!(second_op.value, Some("value2".to_string()));
        assert!(matches!(second_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_squash_operations() {
        // Create two operations that should be squashed: first update, then another update on same property
        let entity1 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value1".to_string(),
                options: None,
            }],
        };

        let entity2 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value2".to_string(),
                options: None,
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity1)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity2)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // Should only create one value operation (the second one should overwrite the first)
        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(created_op.value, Some("value2".to_string()));
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_set_then_delete() {
        // Create an update entity operation followed by an unset operation
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value1".to_string(),
                options: None,
            }],
        };

        let unset = UnsetEntityValues {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            properties: vec![Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                .unwrap()
                .as_bytes()
                .to_vec()],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let op2 = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // Should result in a delete operation (set then delete = net delete)
        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        assert_ne!(deleted[0], Uuid::nil());
    }

    #[test]
    fn test_map_edit_to_values_delete_then_set() {
        // Create an unset entity operation followed by an update operation
        let unset = UnsetEntityValues {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            properties: vec![Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                .unwrap()
                .as_bytes()
                .to_vec()],
        };

        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value1".to_string(),
                options: None,
            }],
        };

        let op1 = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op1, op2]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // Should result in a create operation (delete then set = net set)
        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(created_op.value, Some("value1".to_string()));
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_mixed_operations() {
        // Test a combination of operations
        let entity1 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "value1".to_string(),
                    options: None,
                },
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "value2".to_string(),
                    options: None,
                },
            ],
        };

        let entity2 = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c3")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "value3".to_string(),
                options: None,
            }],
        };

        let unset = UnsetEntityValues {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            properties: vec![Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                .unwrap()
                .as_bytes()
                .to_vec()],
        };

        let op1 = Op {
            payload: Some(Payload::UpdateEntity(entity1)),
        };

        let op2 = Op {
            payload: Some(Payload::UpdateEntity(entity2)),
        };

        let op3 = Op {
            payload: Some(Payload::UnsetEntityValues(unset)),
        };

        let edit = create_test_edit(vec![op1, op2, op3]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // Should create 2 values and delete 1
        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 1);

        // Check the two created values
        let first_created = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
            })
            .unwrap();
        assert_eq!(
            first_created.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(first_created.value, Some("value1".to_string()));

        let second_created = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c3").unwrap()
            })
            .unwrap();
        assert_eq!(
            second_created.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap()
        );
        assert_eq!(second_created.value, Some("value3".to_string()));

        // Check the deleted value
        assert_ne!(deleted[0], Uuid::nil());
    }

    #[test]
    fn test_map_edit_to_values_with_text_options() {
        // Create an update entity operation with text options
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "Hello World".to_string(),
                options: Some(Options {
                    value: Some(options::Value::Text(TextOptions {
                        language: Some("en".as_bytes().to_vec()),
                    })),
                }),
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(
            created_op.space_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(created_op.value, Some("Hello World".to_string()));
        assert_eq!(created_op.language, Some("en".to_string()));
        assert_eq!(created_op.unit, None);
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_with_number_options() {
        // Create a UUID for the number unit
        let unit_uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap();
        let unit_bytes = unit_uuid.as_bytes().to_vec();

        // Create an update entity operation with number options
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "42.5".to_string(),
                options: Some(Options {
                    value: Some(options::Value::Number(NumberOptions {
                        unit: Some(unit_bytes),
                    })),
                }),
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(
            created_op.space_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(created_op.value, Some("42.5".to_string()));
        assert_eq!(created_op.language, None);
        assert_eq!(created_op.unit, Some(unit_uuid.to_string()));
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_with_empty_options() {
        // Create an update entity operation with empty options
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "test value".to_string(),
                options: Some(Options { value: None }),
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(
            created_op.space_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(created_op.value, Some("test value".to_string()));
        assert_eq!(created_op.language, None);
        assert_eq!(created_op.unit, None);
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_with_invalid_utf8_options() {
        // Create an update entity operation with invalid UTF-8 in text options
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "test value".to_string(),
                options: Some(Options {
                    value: Some(options::Value::Text(TextOptions {
                        // This is invalid UTF-8, but should be handled gracefully
                        language: Some(vec![0xC0, 0x80]),
                    })),
                }),
            }],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(
            created_op.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            created_op.property_id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(created_op.value, Some("test value".to_string()));
        // The language should be None for invalid UTF-8
        assert_eq!(created_op.language, None);
        assert_eq!(created_op.unit, None);
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_mixed_options() {
        // Create an update entity operation with mixed option types
        let entity = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "Hello".to_string(),
                    options: Some(Options {
                        value: Some(options::Value::Text(TextOptions {
                            language: Some("fr".as_bytes().to_vec()),
                        })),
                    }),
                },
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "100".to_string(),
                    options: Some(Options {
                        value: Some(options::Value::Number(NumberOptions {
                            unit: Some(
                                Uuid::parse_str("550e8400-e29b-41d4-a716-446655440003")
                                    .unwrap()
                                    .as_bytes()
                                    .to_vec(),
                            ),
                        })),
                    }),
                },
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c3")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "plain".to_string(),
                    options: None,
                },
            ],
        };

        let op = Op {
            payload: Some(Payload::UpdateEntity(entity)),
        };

        let edit = create_test_edit(vec![op]);
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 3);
        assert_eq!(deleted.len(), 0);

        // Find each value by property_id and check its options
        let text_value = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
            })
            .unwrap();
        assert_eq!(text_value.language, Some("fr".to_string()));
        assert_eq!(text_value.unit, None);

        let number_value = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap()
            })
            .unwrap();
        assert_eq!(number_value.language, None);
        assert_eq!(
            number_value.unit,
            Some(
                Uuid::parse_str("550e8400-e29b-41d4-a716-446655440003")
                    .unwrap()
                    .to_string()
            )
        );

        let plain_value = created
            .iter()
            .find(|op| {
                op.property_id == Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c3").unwrap()
            })
            .unwrap();
        assert_eq!(plain_value.language, None);
        assert_eq!(plain_value.unit, None);
    }
}
