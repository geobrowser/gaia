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
    CreatedSpace, PersonalSpace, PublicSpace, KgData,
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
                block,
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
        is_errored: false,
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
                            value: Some("1".to_string()),
                        },
                    ],
                ),
                make_entity_op(
                    TestEntityOpType::UPDATE,
                    "550e8400-e29b-41d4-a716-446655440002",
                    vec![TestValue {
                        property_id: "6ba7b810-9dad-11d1-80b4-00c04fd430c2".to_string(),
                        value: Some("2".to_string()),
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
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Text),
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c2", PbDataType::Number),
            ],
        )),
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
            spaces: vec![],
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

#[tokio::test]
async fn test_validation_rejects_invalid_number() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create a Number property
    let property_id = "11111111-1111-1111-1111-111111111111";
    let property_op = make_property_op(property_id, PbDataType::Number);

    // Try to set an invalid number value (contains letters)
    let invalid_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "22222222-2222-2222-2222-222222222222",
        vec![TestValue {
            property_id: property_id.to_string(),
            value: Some("not_a_number".to_string()),
        }],
    );

    let edit = make_edit(
        "33333333-3333-3333-3333-333333333333",
        "Validation Test Edit",
        "44444444-4444-4444-4444-444444444444",
        vec![property_op, invalid_entity_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap(),
    };

    let kg_data = make_kg_data_with_spaces(10, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer - this should succeed (no crash) but invalid data should be rejected
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage.get_property(&property_id.to_string()).await.unwrap();
    assert_eq!(property.data_type, DataType::Number);

    // Verify the invalid value was NOT stored in the database
    let entity_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(value_result.is_err(), "Invalid number value should not be stored in database");

    Ok(())
}

#[tokio::test]
async fn test_validation_rejects_invalid_checkbox() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create a Checkbox property
    let property_id = "66666666-6666-6666-6666-666666666666";
    let property_op = make_property_op(property_id, PbDataType::Checkbox);

    // Try to set an invalid checkbox value (should be 0 or 1)
    let invalid_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "77777777-7777-7777-7777-777777777777",
        vec![TestValue {
            property_id: property_id.to_string(),
            value: Some("2".to_string()), // Invalid: checkboxes only accept 0 or 1
        }],
    );

    let edit = make_edit(
        "88888888-8888-8888-8888-888888888888",
        "Checkbox Validation Test",
        "99999999-9999-9999-9999-999999999999",
        vec![property_op, invalid_entity_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
    };

    let kg_data = make_kg_data_with_spaces(11, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage.get_property(&property_id.to_string()).await.unwrap();
    assert_eq!(property.data_type, DataType::Checkbox);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("77777777-7777-7777-7777-777777777777").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(value_result.is_err(), "Invalid checkbox value should not be stored in database");

    Ok(())
}

#[tokio::test]
async fn test_validation_rejects_invalid_time() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create a Time property
    let property_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let property_op = make_property_op(property_id, PbDataType::Time);

    // Try to set an invalid time value
    let invalid_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
        vec![TestValue {
            property_id: property_id.to_string(),
            value: Some("not-a-valid-time".to_string()),
        }],
    );

    let edit = make_edit(
        "dddddddd-dddd-dddd-dddd-dddddddddddd",
        "Time Validation Test",
        "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
        vec![property_op, invalid_entity_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap(),
    };

    let kg_data = make_kg_data_with_spaces(12, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage.get_property(&property_id.to_string()).await.unwrap();
    assert_eq!(property.data_type, DataType::Time);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(value_result.is_err(), "Invalid time value should not be stored in database");

    Ok(())
}

#[tokio::test]
async fn test_validation_rejects_invalid_point() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create a Point property
    let property_id = "12345678-1234-1234-1234-123456789012";
    let property_op = make_property_op(property_id, PbDataType::Point);

    // Try to set an invalid point value (should be "x,y" format)
    let invalid_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "23456789-2345-2345-2345-234567890123",
        vec![TestValue {
            property_id: property_id.to_string(),
            value: Some("invalid-point-format".to_string()),
        }],
    );

    let edit = make_edit(
        "34567890-3456-3456-3456-345678901234",
        "Point Validation Test",
        "45678901-4567-4567-4567-456789012345",
        vec![property_op, invalid_entity_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("56789012-5678-5678-5678-567890123456").unwrap(),
    };

    let kg_data = make_kg_data_with_spaces(13, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage.get_property(&property_id.to_string()).await.unwrap();
    assert_eq!(property.data_type, DataType::Point);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("23456789-2345-2345-2345-234567890123").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("56789012-5678-5678-5678-567890123456").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(value_result.is_err(), "Invalid point value should not be stored in database");

    Ok(())
}

#[tokio::test]
async fn test_validation_allows_valid_data_mixed_with_invalid() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create multiple properties
    let number_prop_id = "67890123-6789-6789-6789-678901234567";
    let text_prop_id = "78901234-7890-7890-7890-789012345678";
    
    let number_prop_op = make_property_op(number_prop_id, PbDataType::Number);
    let text_prop_op = make_property_op(text_prop_id, PbDataType::Text);

    // Entity with mixed valid and invalid values
    let mixed_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "89012345-8901-8901-8901-890123456789",
        vec![
            TestValue {
                property_id: number_prop_id.to_string(),
                value: Some("42.5".to_string()), // Valid number
            },
            TestValue {
                property_id: text_prop_id.to_string(),
                value: Some("Valid text".to_string()), // Valid text
            },
        ],
    );

    // Another entity with invalid number but valid text
    let invalid_entity_op = make_entity_op(
        TestEntityOpType::UPDATE,
        "90123456-9012-9012-9012-901234567890",
        vec![
            TestValue {
                property_id: number_prop_id.to_string(),
                value: Some("not_a_number".to_string()), // Invalid number
            },
            TestValue {
                property_id: text_prop_id.to_string(),
                value: Some("Another valid text".to_string()), // Valid text
            },
        ],
    );

    let edit = make_edit(
        "01234567-0123-0123-0123-012345678901",
        "Mixed Validation Test",
        "10987654-1098-1098-1098-109876543210",
        vec![number_prop_op, text_prop_op, mixed_entity_op, invalid_entity_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("21098765-2109-2109-2109-210987654321").unwrap(),
    };

    let kg_data = make_kg_data_with_spaces(14, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify properties were created
    let number_property = storage.get_property(&number_prop_id.to_string()).await.unwrap();
    assert_eq!(number_property.data_type, DataType::Number);
    let text_property = storage.get_property(&text_prop_id.to_string()).await.unwrap();
    assert_eq!(text_property.data_type, DataType::Text);

    let space_id = Uuid::parse_str("21098765-2109-2109-2109-210987654321").unwrap();

    // Check first entity - valid values should be stored
    {
        let entity_id = Uuid::parse_str("89012345-8901-8901-8901-890123456789").unwrap();
        let number_prop_uuid = Uuid::parse_str(number_prop_id).unwrap();
        let text_prop_uuid = Uuid::parse_str(text_prop_id).unwrap();
        
        let number_value_id = derive_value_id(&entity_id, &number_prop_uuid, &space_id);
        let text_value_id = derive_value_id(&entity_id, &text_prop_uuid, &space_id);

        // Both valid values should be stored
        let number_value = storage.get_value(&number_value_id.to_string()).await.unwrap();
        assert_eq!(number_value.value, Some("42.5".to_string()));
        
        let text_value = storage.get_value(&text_value_id.to_string()).await.unwrap();
        assert_eq!(text_value.value, Some("Valid text".to_string()));
    }

    // Check second entity - only valid text should be stored, invalid number should be rejected
    {
        let entity_id = Uuid::parse_str("90123456-9012-9012-9012-901234567890").unwrap();
        let number_prop_uuid = Uuid::parse_str(number_prop_id).unwrap();
        let text_prop_uuid = Uuid::parse_str(text_prop_id).unwrap();
        
        let number_value_id = derive_value_id(&entity_id, &number_prop_uuid, &space_id);
        let text_value_id = derive_value_id(&entity_id, &text_prop_uuid, &space_id);

        // Invalid number should NOT be stored
        let number_value_result = storage.get_value(&number_value_id.to_string()).await;
        assert!(number_value_result.is_err(), "Invalid number should not be stored");
        
        // Valid text should be stored
        let text_value = storage.get_value(&text_value_id.to_string()).await.unwrap();
        assert_eq!(text_value.value, Some("Another valid text".to_string()));
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
            spaces: vec![],
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
            spaces: vec![],
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
            spaces: vec![],
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

// Helper functions for creating spaces
fn make_personal_space(dao_address: &str) -> CreatedSpace {
    CreatedSpace::Personal(PersonalSpace {
        dao_address: dao_address.to_string(),
        space_address: format!("{}_space", dao_address),
        personal_plugin: format!("{}_personal_plugin", dao_address),
    })
}

fn make_public_space(dao_address: &str) -> CreatedSpace {
    CreatedSpace::Public(PublicSpace {
        dao_address: dao_address.to_string(),
        space_address: format!("{}_space", dao_address),
        membership_plugin: format!("{}_membership_plugin", dao_address),
        governance_plugin: format!("{}_governance_plugin", dao_address),
    })
}

fn make_kg_data_with_spaces(
    block_number: u64,
    edits: Vec<PreprocessedEdit>,
    spaces: Vec<CreatedSpace>,
) -> KgData {
    KgData {
        block: BlockMetadata {
            cursor: block_number.to_string(),
            block_number,
            timestamp: "1234567890".to_string(),
        },
        edits,
        spaces,
    }
}

#[tokio::test]
async fn test_space_indexing_personal() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with personal spaces
    let spaces = vec![
        make_personal_space("0x1234567890123456789012345678901234567890"),
        make_personal_space("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd"),
    ];

    let kg_data = make_kg_data_with_spaces(1, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify that spaces were inserted
    // Note: This test verifies that the insertion doesn't fail
    // In a real scenario, you'd query the database to verify the data was inserted correctly
    
    Ok(())
}

#[tokio::test]
async fn test_space_indexing_public() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with public spaces
    let spaces = vec![
        make_public_space("0x9999999999999999999999999999999999999999"),
        make_public_space("0x8888888888888888888888888888888888888888"),
    ];

    let kg_data = make_kg_data_with_spaces(2, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
async fn test_space_indexing_mixed() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with mixed space types
    let spaces = vec![
        make_personal_space("0x1111111111111111111111111111111111111111"),
        make_public_space("0x2222222222222222222222222222222222222222"),
        make_personal_space("0x3333333333333333333333333333333333333333"),
    ];

    let kg_data = make_kg_data_with_spaces(3, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
async fn test_space_indexing_empty() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with no spaces
    let kg_data = make_kg_data_with_spaces(4, vec![], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
async fn test_space_indexing_duplicate_dao_addresses() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with same DAO address for different space types
    let dao_address = "0x5555555555555555555555555555555555555555";
    let spaces = vec![
        make_personal_space(dao_address),
        make_public_space(dao_address),
    ];

    let kg_data = make_kg_data_with_spaces(5, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer - this should work since space IDs are derived differently
    // for personal vs public spaces (even with the same DAO address)
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
async fn test_space_indexing_with_edits() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create some property operations
    let property_id = "1cc6995f-6cc2-4c7a-9592-1466bf95f6be";
    let property_op = make_property_op(property_id, PbDataType::Text);

    // Create a test edit
    let edit = make_edit(
        "08c4f093-7858-4b7c-9b94-b82e448abcff",
        "Test Edit",
        "2cc6995f-6cc2-4c7a-9592-1466bf95f6be",
        vec![property_op],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("3cc6995f-6cc2-4c7a-9592-1466bf95f6be").unwrap(),
    };

    // Create spaces alongside edits
    let spaces = vec![
        make_personal_space("0x6666666666666666666666666666666666666666"),
        make_public_space("0x7777777777777777777777777777777777777777"),
    ];

    let kg_data = make_kg_data_with_spaces(6, vec![item], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}
