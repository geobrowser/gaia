use grc20::pb::grc20::{
    op::Payload, DataType as PbDataType, Edit, Entity, Op, Property, Relation, UnsetEntityValues,
    Value,
};
use std::{
    collections::hash_map::DefaultHasher,
    env,
    hash::{Hash, Hasher},
    sync::Arc,
};
use stream::utils::BlockMetadata;
use uuid::Uuid;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{properties_cache::PropertiesCache, PreprocessedEdit},
    error::IndexingError,
    models::properties::DataType,
    storage::postgres::PostgresStorage,
    KgData,
};

struct TestIndexer {
    storage: Arc<PostgresStorage>,
    properties_cache: Arc<PropertiesCache>,
}

impl TestIndexer {
    pub fn new(storage: Arc<PostgresStorage>, properties_cache: Arc<PropertiesCache>) -> Self {
        TestIndexer {
            storage,
            properties_cache,
        }
    }

    pub async fn run(&self, blocks: &Vec<KgData>) -> Result<(), IndexingError> {
        for block in blocks {
            root_handler::run(
                block.edits.clone(),
                &block.block,
                &self.storage,
                &self.properties_cache,
            )
            .await?;
        }

        Ok(())
    }
}

// @TODO: Different test for the cache preprocessing

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);

    let item = PreprocessedEdit {
        space_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440007").unwrap(),
        edit: Some(make_edit(
            "f47ac10b-58cc-4372-a567-0e02b2c3d479",
            "Name",
            "f47ac10b-58cc-4372-a567-0e02b2c3d480",
            vec![
                make_entity_op(
                    TestEntityOpType::UPDATE,
                    "550e8400-e29b-41d4-a716-446655440001",
                    vec![
                        TestValue {
                            property_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c1".to_string(),
                            value: Some("Test entity".to_string()),
                        },
                        TestValue {
                            property_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string(),
                            value: Some("value 1".to_string()),
                        },
                    ],
                ),
                make_entity_op(
                    TestEntityOpType::UPDATE,
                    "550e8400-e29b-41d4-a716-446655440002",
                    vec![TestValue {
                        property_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string(),
                        value: Some("value 2".to_string()),
                    }],
                ),
                make_entity_op(
                    TestEntityOpType::UNSET,
                    "550e8400-e29b-41d4-a716-446655440002",
                    vec![TestValue {
                        property_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string(),
                        value: None,
                    }],
                ),
                make_relation_op(
                    TestRelationOpType::CREATE,
                    "7ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440001",
                    "8ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440003",
                    "550e8400-e29b-41d4-a716-446655440004",
                ),
                make_relation_op(
                    TestRelationOpType::UPDATE,
                    "7ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440001",
                    "8ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440003",
                    "550e8400-e29b-41d4-a716-446655440004",
                ),
                make_relation_op(
                    TestRelationOpType::CREATE,
                    "7ba7b810-9dad-11d1-80b4-00c04fd430c2",
                    "550e8400-e29b-41d4-a716-446655440001",
                    "8ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440003",
                    "550e8400-e29b-41d4-a716-446655440004",
                ),
                make_relation_op(
                    TestRelationOpType::DELETE,
                    "7ba7b810-9dad-11d1-80b4-00c04fd430c2",
                    "550e8400-e29b-41d4-a716-446655440001",
                    "8ba7b810-9dad-11d1-80b4-00c04fd430c1",
                    "550e8400-e29b-41d4-a716-446655440003",
                    "550e8400-e29b-41d4-a716-446655440004",
                ),
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c1", NativeTypes::Text),
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c2", NativeTypes::Number),
            ],
        )),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("5"),
        block_number: 1,
        timestamp: String::from("5"),
    };

    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache.clone());

    indexer
        .run(&vec![KgData {
            block,
            edits: vec![item],
        }])
        .await?;

    {
        let entity = storage
            .get_entity(&"550e8400-e29b-41d4-a716-446655440001".to_string())
            .await
            .unwrap();
        assert_eq!(
            entity.id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
    }

    {
        let entity = storage
            .get_entity(&"550e8400-e29b-41d4-a716-446655440002".to_string())
            .await
            .unwrap();
        assert_eq!(
            entity.id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap()
        );
    }

    {
        let entity_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap();
        let property_id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap();
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440007").unwrap();
        let expected_value_id = derive_value_id(&entity_id, &property_id, &space_id);

        let value = storage
            .get_value(&expected_value_id.to_string())
            .await
            .unwrap();
        assert_eq!(value.id, expected_value_id);
    }

    {
        let entity_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440002").unwrap();
        let property_id = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap();
        let space_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440007").unwrap();
        let expected_value_id = derive_value_id(&entity_id, &property_id, &space_id);

        let value = storage.get_value(&expected_value_id.to_string()).await;

        // Should not return the value since it was deleted
        assert_eq!(value.is_err(), true);
    }

    {
        let relation = storage
            .get_relation(&"7ba7b810-9dad-11d1-80b4-00c04fd430c1".to_string())
            .await
            .unwrap();

        assert_eq!(
            relation.id,
            Uuid::parse_str("7ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(
            relation.space_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440007").unwrap()
        );
        assert_eq!(
            relation.entity_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
        );
        assert_eq!(
            relation.from_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440003").unwrap()
        );
        assert_eq!(
            relation.to_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440004").unwrap()
        );

        // Update in edit sets verified to Some(true)
        assert_eq!(relation.verified, Some(true));
    }

    {
        // Should not return the value since it was deleted
        let value = storage
            .get_relation(&"7ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string())
            .await;
        assert_eq!(value.is_err(), true);
    }

    // Test property creation
    {
        let property = storage
            .get_property(&"6ba7b810-9dad-11d1-80b4-00c04fd430c1".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(property.data_type, DataType::Text);
    }

    {
        let property = storage
            .get_property(&"6ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap()
        );
        assert_eq!(property.data_type, DataType::Number);
    }

    Ok(())
}

fn derive_value_id(entity_id: &Uuid, property_id: &Uuid, space_id: &Uuid) -> Uuid {
    let mut hasher = DefaultHasher::new();
    entity_id.hash(&mut hasher);
    property_id.hash(&mut hasher);
    space_id.hash(&mut hasher);
    let hash_value = hasher.finish();

    // Create a deterministic UUID from the hash
    let mut bytes = [0u8; 16];
    bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
    bytes[8..16].copy_from_slice(&hash_value.to_be_bytes());

    Uuid::from_bytes(bytes)
}

#[tokio::test]
async fn test_property_no_overwrite() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);

    // First edit - create property with Text type
    let item = PreprocessedEdit {
        space_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440005").unwrap(),
        edit: Some(make_edit(
            "f47ac10b-58cc-4372-a567-0e02b2c3d481",
            "First Edit",
            "f47ac10b-58cc-4372-a567-0e02b2c3d480",
            vec![make_property_op(
                "aba7b810-9dad-11d1-80b4-00c04fd430c1",
                PbDataType::Text,
            )],
        )),
        is_errored: false,
    };

    // Second edit - attempt to create same property with Number type
    let second_edit = PreprocessedEdit {
        space_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440006").unwrap(),
        edit: Some(make_edit(
            "f47ac10b-58cc-4372-a567-0e02b2c3d482",
            "Second Edit",
            "f47ac10b-58cc-4372-a567-0e02b2c3d480",
            vec![make_property_op(
                "aba7b810-9dad-11d1-80b4-00c04fd430c1",
                PbDataType::Number,
            )],
        )),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("6"),
        block_number: 2,
        timestamp: String::from("6"),
    };

    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache.clone());

    // Process first edit
    indexer
        .run(&vec![KgData {
            block: block.clone(),
            edits: vec![item],
        }])
        .await?;

    // Verify property was created with Text type
    {
        let property = storage
            .get_property(&"aba7b810-9dad-11d1-80b4-00c04fd430c1".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("aba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(property.data_type, DataType::Text);
    }

    // Process second edit (should not overwrite)
    indexer
        .run(&vec![KgData {
            block,
            edits: vec![second_edit],
        }])
        .await?;

    // Verify property still has Text type (not overwritten)
    {
        let property = storage
            .get_property(&"aba7b810-9dad-11d1-80b4-00c04fd430c1".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("aba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(property.data_type, DataType::Text); // Should still be Text, not Number
    }

    Ok(())
}

#[tokio::test]
async fn test_property_squashing() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);

    // Single edit with multiple CreateProperty ops for the same property ID
    let edit_with_duplicate_properties = PreprocessedEdit {
        space_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440008").unwrap(),
        edit: Some(make_edit(
            "f47ac10b-58cc-4372-a567-0e02b2c3d483",
            "Squash Test Edit",
            "f47ac10b-58cc-4372-a567-0e02b2c3d480",
            vec![
                // First: create property with Text type
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Text),
                // Second: create same property with Number type
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Number),
                // Third: create same property with Checkbox type (this should be the final one)
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Checkbox),
                // Different property to ensure squashing only affects same IDs
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c2", PbDataType::Time),
            ],
        )),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("7"),
        block_number: 3,
        timestamp: String::from("7"),
    };

    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache.clone());

    // Process the edit
    indexer
        .run(&vec![KgData {
            block,
            edits: vec![edit_with_duplicate_properties],
        }])
        .await?;

    // Verify that only the final type (Checkbox) was stored for the squashed property
    {
        let property = storage
            .get_property(&"bba7b810-9dad-11d1-80b4-00c04fd430c1".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("bba7b810-9dad-11d1-80b4-00c04fd430c1").unwrap()
        );
        assert_eq!(property.data_type, DataType::Checkbox); // Should be Checkbox, not Text or Number
    }

    // Verify that the different property was not affected by squashing
    {
        let property = storage
            .get_property(&"bba7b810-9dad-11d1-80b4-00c04fd430c2".to_string())
            .await
            .unwrap();
        assert_eq!(
            property.id,
            Uuid::parse_str("bba7b810-9dad-11d1-80b4-00c04fd430c2").unwrap()
        );
        assert_eq!(property.data_type, DataType::Time);
    }

    Ok(())
}

fn make_edit(id: &str, name: &str, author: &str, ops: Vec<Op>) -> Edit {
    Edit {
        id: Uuid::parse_str(id).unwrap().as_bytes().to_vec(),
        name: String::from(name),
        ops,
        authors: vec![Uuid::parse_str(author).unwrap().as_bytes().to_vec()],
        language: None,
    }
}

struct TestValue {
    pub property_id: String,
    pub value: Option<String>,
}

enum TestEntityOpType {
    UPDATE,
    UNSET,
}

fn make_entity_op(op_type: TestEntityOpType, entity: &str, values: Vec<TestValue>) -> Op {
    match op_type {
        TestEntityOpType::UPDATE => Op {
            payload: Some(Payload::UpdateEntity(Entity {
                id: Uuid::parse_str(entity).unwrap().as_bytes().to_vec(),
                values: values
                    .iter()
                    .map(|v| Value {
                        property: Uuid::parse_str(&v.property_id).unwrap().as_bytes().to_vec(),
                        value: v.value.clone().unwrap(),
                        options: None,
                    })
                    .collect(),
            })),
        },
        TestEntityOpType::UNSET => Op {
            payload: Some(Payload::UnsetEntityValues(UnsetEntityValues {
                id: Uuid::parse_str(entity).unwrap().as_bytes().to_vec(),
                properties: values
                    .iter()
                    .map(|v| Uuid::parse_str(&v.property_id).unwrap().as_bytes().to_vec())
                    .collect(),
            })),
        },
    }
}

fn make_property_op(property_id: &str, property_type: PbDataType) -> Op {
    Op {
        payload: Some(Payload::CreateProperty(Property {
            id: Uuid::parse_str(property_id).unwrap().as_bytes().to_vec(),
            data_type: property_type as i32,
        })),
    }
}

enum TestRelationOpType {
    CREATE,
    UPDATE,
    DELETE,
}

fn make_relation_op(
    op_type: TestRelationOpType,
    relation_id: &str,
    entity_id: &str,
    type_id: &str,
    from_entity: &str,
    to_entity: &str,
) -> Op {
    match op_type {
        TestRelationOpType::CREATE => Op {
            payload: Some(Payload::CreateRelation(Relation {
                id: Uuid::parse_str(relation_id).unwrap().as_bytes().to_vec(),
                r#type: Uuid::parse_str(type_id).unwrap().as_bytes().to_vec(),
                entity: Uuid::parse_str(entity_id).unwrap().as_bytes().to_vec(),
                from_entity: Uuid::parse_str(from_entity).unwrap().as_bytes().to_vec(),
                from_space: None,
                from_version: None,
                to_entity: Uuid::parse_str(to_entity).unwrap().as_bytes().to_vec(),
                to_space: None,
                to_version: None,
                position: None,
                verified: None,
            })),
        },
        TestRelationOpType::UPDATE => Op {
            payload: Some(Payload::UpdateRelation(grc20::pb::grc20::RelationUpdate {
                id: Uuid::parse_str(relation_id).unwrap().as_bytes().to_vec(),
                from_space: None,
                from_version: None,
                to_space: None,
                to_version: None,
                position: None,
                verified: Some(true),
            })),
        },
        TestRelationOpType::DELETE => Op {
            payload: Some(Payload::DeleteRelation(
                Uuid::parse_str(relation_id).unwrap().as_bytes().to_vec(),
            )),
        },
    }
}
