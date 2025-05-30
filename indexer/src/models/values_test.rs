use crate::models::values::{ValueChangeType, ValuesModel};
use grc20::pb::grc20::{op::Payload, Edit, Entity, Op, UnsetEntityValues, Value, Options, TextOptions, NumberOptions, options};
use uuid::Uuid;

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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(created_op.entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created_op.property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created_op.space_id, "space1");
        assert_eq!(created_op.value, Some("value1".to_string()));
        assert_eq!(
            created_op.id,
            "550e8400-e29b-41d4-a716-446655440001:6ba7b810-9dad-11d1-80b4-00c04fd430c1:space1"
        );
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        assert_eq!(
            deleted[0],
            "550e8400-e29b-41d4-a716-446655440001:6ba7b810-9dad-11d1-80b4-00c04fd430c1:space1"
        );
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Sort by property_id to make test deterministic
        let mut created = created;
        created.sort_by(|a, b| a.property_id.cmp(&b.property_id));

        assert_eq!(
            created[0].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created[0].value, Some("value1".to_string()));
        assert_eq!(
            created[1].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c2"
        );
        assert_eq!(created[1].value, Some("value2".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_multiple_entities() {
        // Create operations for multiple entities
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 2);
        assert_eq!(deleted.len(), 0);

        // Sort by entity_id to make test deterministic
        let mut created = created;
        created.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));

        assert_eq!(created[0].entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created[0].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created[0].value, Some("value1".to_string()));

        assert_eq!(created[1].entity_id, "550e8400-e29b-41d4-a716-446655440002");
        assert_eq!(
            created[1].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created[1].value, Some("value2".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_squash_operations() {
        // Test the squashing behavior where multiple operations target the same entity property
        // First create a SET operation
        let entity_set = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "initial_value".to_string(),
                options: None,
            }],
        };

        // Then create another SET operation that updates the same property
        let entity_update = Entity {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            values: vec![Value {
                property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
                value: "updated_value".to_string(),
                options: None,
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
        assert_eq!(created[0].entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created[0].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created[0].value, Some("updated_value".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_set_then_delete() {
        // Test a SET followed by a DELETE for the same entity/property
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // After squashing, we should only have the DELETE operation
        assert_eq!(created.len(), 0);
        assert_eq!(deleted.len(), 1);
        assert_eq!(
            deleted[0],
            "550e8400-e29b-41d4-a716-446655440001:6ba7b810-9dad-11d1-80b4-00c04fd430c1:space1"
        );
    }

    #[test]
    fn test_map_edit_to_values_delete_then_set() {
        // Test a DELETE followed by a SET for the same entity/property
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        // After squashing, we should only have the SET operation
        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        assert_eq!(created[0].entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created[0].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created[0].value, Some("value1".to_string()));
    }

    #[test]
    fn test_map_edit_to_values_mixed_operations() {
        // Test a mix of operations for different entities and properties
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

        let unset1 = UnsetEntityValues {
            id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001")
                .unwrap()
                .as_bytes()
                .to_vec(),
            properties: vec![Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1")
                .unwrap()
                .as_bytes()
                .to_vec()],
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

        assert_eq!(created[0].entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created[0].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c2"
        );
        assert_eq!(created[0].value, Some("value2".to_string()));

        assert_eq!(created[1].entity_id, "550e8400-e29b-41d4-a716-446655440002");
        assert_eq!(
            created[1].property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c3"
        );
        assert_eq!(created[1].value, Some("value3".to_string()));

        assert_eq!(
            deleted[0],
            "550e8400-e29b-41d4-a716-446655440001:6ba7b810-9dad-11d1-80b4-00c04fd430c1:space1"
        );
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(created_op.entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created_op.property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created_op.space_id, "space1");
        assert_eq!(created_op.value, Some("Hello World".to_string()));
        assert_eq!(created_op.language, Some("en".to_string()));
        assert_eq!(created_op.unit, None);
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_with_number_options() {
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
                        unit: Some("kg".as_bytes().to_vec()),
                    })),
                }),
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
        assert_eq!(created_op.entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created_op.property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created_op.space_id, "space1");
        assert_eq!(created_op.value, Some("42.5".to_string()));
        assert_eq!(created_op.language, None);
        assert_eq!(created_op.unit, Some("kg".to_string()));
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 1);
        assert_eq!(deleted.len(), 0);

        let created_op = &created[0];
        assert_eq!(created_op.entity_id, "550e8400-e29b-41d4-a716-446655440001");
        assert_eq!(
            created_op.property_id,
            "6ba7b810-9dad-11d1-80b4-00c04fd430c1"
        );
        assert_eq!(created_op.space_id, "space1");
        assert_eq!(created_op.value, Some("test value".to_string()));
        assert_eq!(created_op.language, None);
        assert_eq!(created_op.unit, None);
        assert!(matches!(created_op.change_type, ValueChangeType::SET));
    }

    #[test]
    fn test_map_edit_to_values_with_invalid_utf8_options() {
        // Create an update entity operation with invalid UTF-8 in options (should be ignored)
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
                    value: "text value".to_string(),
                    options: Some(Options {
                        value: Some(options::Value::Text(TextOptions {
                            language: Some(vec![0xff, 0xfe, 0xfd]), // Invalid UTF-8
                        })),
                    }),
                },
                Value {
                    property: Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2")
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                    value: "number value".to_string(),
                    options: Some(Options {
                        value: Some(options::Value::Number(NumberOptions {
                            unit: Some(vec![0xff, 0xfe, 0xfd]), // Invalid UTF-8
                        })),
                    }),
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

        // Both values should have None for language/unit due to invalid UTF-8
        for created_op in &created {
            assert_eq!(created_op.language, None);
            assert_eq!(created_op.unit, None);
            assert!(matches!(created_op.change_type, ValueChangeType::SET));
        }
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
                            unit: Some("m".as_bytes().to_vec()),
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
        let space_id = "space1".to_string();

        let (created, deleted) = ValuesModel::map_edit_to_values(&edit, &space_id);

        assert_eq!(created.len(), 3);
        assert_eq!(deleted.len(), 0);

        // Find each value by property_id and check its options
        let text_value = created
            .iter()
            .find(|op| op.property_id == "6ba7b810-9dad-11d1-80b4-00c04fd430c1")
            .unwrap();
        assert_eq!(text_value.language, Some("fr".to_string()));
        assert_eq!(text_value.unit, None);

        let number_value = created
            .iter()
            .find(|op| op.property_id == "6ba7b810-9dad-11d1-80b4-00c04fd430c2")
            .unwrap();
        assert_eq!(number_value.language, None);
        assert_eq!(number_value.unit, Some("m".to_string()));

        let plain_value = created
            .iter()
            .find(|op| op.property_id == "6ba7b810-9dad-11d1-80b4-00c04fd430c3")
            .unwrap();
        assert_eq!(plain_value.language, None);
        assert_eq!(plain_value.unit, None);
    }
}
