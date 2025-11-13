use std::{
    collections::hash_map::DefaultHasher,
    env,
    hash::{Hash, Hasher},
    sync::Arc,
};
use stream::utils::BlockMetadata;
use uuid::Uuid;
use wire::pb::grc20::{
    op::Payload, DataType as PbDataType, Edit, Entity, Op, Property, Relation, UnsetEntityValues,
    Value,
};

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{properties_cache::{PropertiesCache, ImmutableCache}, PreprocessedEdit},
    error::IndexingError,
    models::properties::DataType,
    storage::{postgres::PostgresStorage, StorageError},
    test_utils::TestStorage,
    AddedMember, AddedSubspace, CreatedSpace, KgData, PersonalSpace, PublicSpace, RemovedMember,
    RemovedSubspace,
};
use indexer_utils::{checksum_address, id::derive_space_id, network_ids::GEO};
use serial_test::serial;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
            root_handler::run(block, &block.block, &self.storage, &self.properties_cache).await?;
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
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::String),
                make_property_op("6ba7b810-9dad-11d1-80b4-00c04fd430c2", PbDataType::Number),
            ],
        )),
        cid: "".to_string(),
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
            added_editors: vec![],
            added_members: vec![],
            removed_editors: vec![],
            removed_members: vec![],
            added_subspaces: vec![],
            removed_subspaces: vec![],
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
        assert_eq!(property.data_type, DataType::String);
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
#[serial]
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
        cid: "".to_string(),
    };

    let kg_data = make_kg_data_with_spaces(10, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer - this should succeed (no crash) but invalid data should be rejected
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage
        .get_property(&property_id.to_string())
        .await
        .unwrap();
    assert_eq!(property.data_type, DataType::Number);

    // Verify the invalid value was NOT stored in the database
    let entity_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(
        value_result.is_err(),
        "Invalid number value should not be stored in database"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_validation_rejects_invalid_checkbox() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create a Checkbox property
    let property_id = "66666666-6666-6666-6666-666666666666";
    let property_op = make_property_op(property_id, PbDataType::Boolean);

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
        cid: "".to_string(),
    };

    let kg_data = make_kg_data_with_spaces(11, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage
        .get_property(&property_id.to_string())
        .await
        .unwrap();
    assert_eq!(property.data_type, DataType::Boolean);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("77777777-7777-7777-7777-777777777777").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(
        value_result.is_err(),
        "Invalid checkbox value should not be stored in database"
    );

    Ok(())
}

#[tokio::test]
#[serial]
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
        cid: "".to_string(),
    };

    let kg_data = make_kg_data_with_spaces(12, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage
        .get_property(&property_id.to_string())
        .await
        .unwrap();
    assert_eq!(property.data_type, DataType::Time);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(
        value_result.is_err(),
        "Invalid time value should not be stored in database"
    );

    Ok(())
}

#[tokio::test]
#[serial]
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
        cid: "".to_string(),
    };

    let kg_data = make_kg_data_with_spaces(13, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify the property was created
    let property = storage
        .get_property(&property_id.to_string())
        .await
        .unwrap();
    assert_eq!(property.data_type, DataType::Point);

    // Verify the invalid value was NOT stored
    let entity_id = Uuid::parse_str("23456789-2345-2345-2345-234567890123").unwrap();
    let property_id_uuid = Uuid::parse_str(property_id).unwrap();
    let space_id = Uuid::parse_str("56789012-5678-5678-5678-567890123456").unwrap();
    let expected_value_id = derive_value_id(&entity_id, &property_id_uuid, &space_id);

    let value_result = storage.get_value(&expected_value_id.to_string()).await;
    assert!(
        value_result.is_err(),
        "Invalid point value should not be stored in database"
    );

    Ok(())
}

#[tokio::test]
#[serial]
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
    let text_prop_op = make_property_op(text_prop_id, PbDataType::String);

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
        vec![
            number_prop_op,
            text_prop_op,
            mixed_entity_op,
            invalid_entity_op,
        ],
    );

    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("21098765-2109-2109-2109-210987654321").unwrap(),
        cid: "".to_string(),
    };

    let kg_data = make_kg_data_with_spaces(14, vec![item], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify properties were created
    let number_property = storage
        .get_property(&number_prop_id.to_string())
        .await
        .unwrap();
    assert_eq!(number_property.data_type, DataType::Number);
    let text_property = storage
        .get_property(&text_prop_id.to_string())
        .await
        .unwrap();
    assert_eq!(text_property.data_type, DataType::String);

    let space_id = Uuid::parse_str("21098765-2109-2109-2109-210987654321").unwrap();

    // Check first entity - valid values should be stored
    {
        let entity_id = Uuid::parse_str("89012345-8901-8901-8901-890123456789").unwrap();
        let number_prop_uuid = Uuid::parse_str(number_prop_id).unwrap();
        let text_prop_uuid = Uuid::parse_str(text_prop_id).unwrap();

        let number_value_id = derive_value_id(&entity_id, &number_prop_uuid, &space_id);
        let text_value_id = derive_value_id(&entity_id, &text_prop_uuid, &space_id);

        // Both valid values should be stored
        let number_value = storage
            .get_value(&number_value_id.to_string())
            .await
            .unwrap();
        assert_eq!(number_value.number, Some(42.5));

        let text_value = storage.get_value(&text_value_id.to_string()).await.unwrap();
        assert_eq!(text_value.string, Some("Valid text".to_string()));
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
        assert!(
            number_value_result.is_err(),
            "Invalid number should not be stored"
        );

        // Valid text should be stored
        let text_value = storage.get_value(&text_value_id.to_string()).await.unwrap();
        assert_eq!(text_value.string, Some("Another valid text".to_string()));
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
#[serial]
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
                PbDataType::String,
            )],
        )),
        is_errored: false,
        cid: "".to_string(),
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
        cid: "".to_string(),
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
            added_editors: vec![],
            added_members: vec![],
            removed_editors: vec![],
            removed_members: vec![],
            added_subspaces: vec![],
            removed_subspaces: vec![],
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
        assert_eq!(property.data_type, DataType::String);
    }

    // Process second edit (should not overwrite)
    indexer
        .run(&vec![KgData {
            block,
            edits: vec![second_edit],
            spaces: vec![],
            added_editors: vec![],
            added_members: vec![],
            removed_editors: vec![],
            removed_members: vec![],
            added_subspaces: vec![],
            removed_subspaces: vec![],
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
        assert_eq!(property.data_type, DataType::String); // Should still be Text, not Number
    }

    Ok(())
}

#[tokio::test]
#[serial]
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
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::String),
                // Second: create same property with Number type
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Number),
                // Third: create same property with Checkbox type (this should be the final one)
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c1", PbDataType::Boolean),
                // Different property to ensure squashing only affects same IDs
                make_property_op("bba7b810-9dad-11d1-80b4-00c04fd430c2", PbDataType::Time),
            ],
        )),
        is_errored: false,
        cid: "".to_string(),
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
            added_editors: vec![],
            added_members: vec![],
            removed_editors: vec![],
            removed_members: vec![],
            added_subspaces: vec![],
            removed_subspaces: vec![],
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
        assert_eq!(property.data_type, DataType::Boolean); // Should be Checkbox, not Text or Number
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
            payload: Some(Payload::UpdateRelation(wire::pb::grc20::RelationUpdate {
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
        added_editors: vec![],
        added_members: vec![],
        removed_editors: vec![],
        removed_members: vec![],
        added_subspaces: vec![],
        removed_subspaces: vec![],
    }
}

#[tokio::test]
#[serial]
async fn test_space_indexing_personal() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the spaces table to ensure clean test state
    test_storage.clear_table("spaces").await?;

    // Create test data with personal spaces
    let dao_address1 = generate_unique_address("personal_space_test_1");
    let dao_address2 = generate_unique_address("personal_space_test_2");

    let spaces = vec![
        make_personal_space(&dao_address1),
        make_personal_space(&dao_address2),
    ];

    let kg_data = make_kg_data_with_spaces(1, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify that personal spaces were inserted correctly
    // Need to checksum addresses since they are stored checksummed in the database
    use indexer_utils::checksum_address;
    let dao_addresses = vec![
        checksum_address(&dao_address1),
        checksum_address(&dao_address2),
    ];
    let space_rows = test_storage
        .get_spaces_by_dao_addresses(&dao_addresses)
        .await?;

    assert_eq!(space_rows.len(), 2);

    // Create expected personal plugin addresses the same way the production code does
    let expected_personal_addresses = vec![
        checksum_address(&format!("{}_personal_plugin", dao_address1)),
        checksum_address(&format!("{}_personal_plugin", dao_address2)),
    ];
    let expected_space_addresses = vec![
        checksum_address(&format!("{}_space", dao_address1)),
        checksum_address(&format!("{}_space", dao_address2)),
    ];

    for row in &space_rows {
        row.validate_personal_space().map_err(|_e| {
            IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound))
        })?;

        // Verify the personal_address matches one of the expected addresses
        let personal_addr = row.personal_address.as_ref().unwrap();
        assert!(
            expected_personal_addresses.contains(personal_addr),
            "Personal address {} not found in expected addresses",
            personal_addr
        );

        // Verify the space_address matches one of the expected addresses
        assert!(
            expected_space_addresses.contains(&row.space_address),
            "Space address {} not found in expected addresses",
            row.space_address
        );
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_space_indexing_public() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the spaces table to ensure clean test state
    test_storage.clear_table("spaces").await?;

    // Create test data with public spaces
    let dao_address1 = generate_unique_address("public_space_test_1");
    let dao_address2 = generate_unique_address("public_space_test_2");
    let spaces = vec![
        make_public_space(&dao_address1),
        make_public_space(&dao_address2),
    ];

    let kg_data = make_kg_data_with_spaces(2, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify that public spaces were inserted correctly
    // Need to checksum addresses since they are stored checksummed in the database
    use indexer_utils::checksum_address;
    let dao_addresses = vec![
        checksum_address(&dao_address1),
        checksum_address(&dao_address2),
    ];
    let space_rows = test_storage
        .get_spaces_by_dao_addresses(&dao_addresses)
        .await?;

    assert_eq!(space_rows.len(), 2);

    // Create expected addresses the same way the production code does
    let expected_governance_addresses = vec![
        checksum_address(&format!("{}_governance_plugin", dao_address1)),
        checksum_address(&format!("{}_governance_plugin", dao_address2)),
    ];
    let expected_membership_addresses = vec![
        checksum_address(&format!("{}_membership_plugin", dao_address1)),
        checksum_address(&format!("{}_membership_plugin", dao_address2)),
    ];
    let expected_space_addresses = vec![
        checksum_address(&format!("{}_space", dao_address1)),
        checksum_address(&format!("{}_space", dao_address2)),
    ];

    for row in &space_rows {
        row.validate_public_space().map_err(|_e| {
            IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound))
        })?;

        // Verify the governance address matches one of the expected addresses
        let governance_addr = row.main_voting_address.as_ref().unwrap();
        assert!(
            expected_governance_addresses.contains(governance_addr),
            "Governance address {} not found in expected addresses",
            governance_addr
        );

        // Verify the membership address matches one of the expected addresses
        let membership_addr = row.membership_address.as_ref().unwrap();
        assert!(
            expected_membership_addresses.contains(membership_addr),
            "Membership address {} not found in expected addresses",
            membership_addr
        );

        // Verify the space_address matches one of the expected addresses
        assert!(
            expected_space_addresses.contains(&row.space_address),
            "Space address {} not found in expected addresses",
            row.space_address
        );
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_space_indexing_mixed() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the spaces table to ensure clean test state
    test_storage.clear_table("spaces").await?;

    // Create test data with mixed space types
    let personal_dao1 = generate_unique_address("mixed_space_test_personal1");
    let public_dao = generate_unique_address("mixed_space_test_public");
    let personal_dao2 = generate_unique_address("mixed_space_test_personal2");
    let spaces = vec![
        make_personal_space(&personal_dao1),
        make_public_space(&public_dao),
        make_personal_space(&personal_dao2),
    ];

    let kg_data = make_kg_data_with_spaces(3, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify that mixed space types were inserted correctly
    // Need to checksum addresses since they are stored checksummed in the database
    use indexer_utils::checksum_address;
    let dao_addresses = vec![
        checksum_address(&personal_dao1),
        checksum_address(&public_dao),
        checksum_address(&personal_dao2),
    ];
    let space_rows = test_storage
        .get_spaces_by_dao_addresses(&dao_addresses)
        .await?;

    assert_eq!(space_rows.len(), 3);

    // Check personal space 1
    let checksummed_personal_dao1 = checksum_address(&personal_dao1);
    let personal_row1 = space_rows
        .iter()
        .find(|r| r.dao_address == checksummed_personal_dao1)
        .unwrap();
    personal_row1.validate_personal_space().map_err(|_e| {
        IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound))
    })?;

    // Check public space
    let checksummed_public_dao = checksum_address(&public_dao);
    let public_row = space_rows
        .iter()
        .find(|r| r.dao_address == checksummed_public_dao)
        .unwrap();
    public_row.validate_public_space().map_err(|_e| {
        IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound))
    })?;

    // Check personal space 2
    let checksummed_personal_dao2 = checksum_address(&personal_dao2);
    let personal_row2 = space_rows
        .iter()
        .find(|r| r.dao_address == checksummed_personal_dao2)
        .unwrap();
    personal_row2.validate_personal_space().map_err(|_e| {
        IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound))
    })?;

    Ok(())
}

#[tokio::test]
#[serial]
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

fn generate_unique_address(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Get a unique counter value for this call
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);

    // Create a hash from prefix to get deterministic but unique start
    let mut hasher = DefaultHasher::new();
    prefix.hash(&mut hasher);
    counter.hash(&mut hasher); // Include counter in hash for extra uniqueness
    timestamp.hash(&mut hasher); // Include timestamp in hash
    let prefix_hash = hasher.finish();

    // Combine prefix hash, timestamp, and counter to create exactly 40 hex characters
    let part1 = prefix_hash & 0xFFFFFFFFFFFFFFFF;
    let part2 = (timestamp ^ (counter as u128)) & 0xFFFFFFFFFFFFFFFF;
    let part3 = ((timestamp >> 64) ^ (counter as u128)) & 0xFFFFFFFF;

    format!("0x{:016x}{:016x}{:08x}", part1, part2 as u64, part3 as u32)
}

fn make_added_member(dao_address: &str, editor_address: &str) -> AddedMember {
    AddedMember {
        dao_address: dao_address.to_string(),
        editor_address: editor_address.to_string(),
    }
}

fn make_removed_member(dao_address: &str, editor_address: &str) -> RemovedMember {
    RemovedMember {
        dao_address: dao_address.to_string(),
        editor_address: editor_address.to_string(),
    }
}

fn make_kg_data_with_membership(
    block_number: u64,
    added_members: Vec<AddedMember>,
    removed_members: Vec<RemovedMember>,
    added_editors: Vec<AddedMember>,
    removed_editors: Vec<RemovedMember>,
) -> KgData {
    KgData {
        block: BlockMetadata {
            cursor: block_number.to_string(),
            block_number,
            timestamp: "1234567890".to_string(),
        },
        edits: vec![],
        spaces: vec![],
        added_members,
        removed_members,
        added_editors,
        removed_editors,
        added_subspaces: vec![],
        removed_subspaces: vec![],
    }
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_added_members() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the members table to ensure clean test state
    test_storage.clear_table("members").await?;

    let dao_address = generate_unique_address("add_members_test_dao");
    let member_address1 = generate_unique_address("add_members_test_mem1");
    let member_address2 = generate_unique_address("add_members_test_mem2");

    let added_members = vec![
        make_added_member(&dao_address, &member_address1),
        make_added_member(&dao_address, &member_address2),
    ];

    let kg_data = make_kg_data_with_membership(1, added_members, vec![], vec![], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify members were inserted
    let space_id = derive_space_id(GEO, &checksum_address(dao_address.to_string()));
    let member1 = indexer
        .storage
        .get_member(&checksum_address(member_address1.to_string()), &space_id)
        .await;
    let member2 = indexer
        .storage
        .get_member(&checksum_address(member_address2.to_string()), &space_id)
        .await;

    assert!(member1.is_ok());
    assert!(member2.is_ok());

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_added_editors() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the editors table to ensure clean test state
    test_storage.clear_table("editors").await?;

    let dao_address = generate_unique_address("add_editors_test_dao");
    let editor_address1 = generate_unique_address("add_editors_test_edit1");
    let editor_address2 = generate_unique_address("add_editors_test_edit2");

    let added_editors = vec![
        make_added_member(&dao_address, &editor_address1),
        make_added_member(&dao_address, &editor_address2),
    ];

    let kg_data = make_kg_data_with_membership(1, vec![], vec![], added_editors, vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify editors were inserted
    let space_id = derive_space_id(GEO, &checksum_address(dao_address.to_string()));
    let editor1 = indexer
        .storage
        .get_editor(&checksum_address(editor_address1.to_string()), &space_id)
        .await;
    let editor2 = indexer
        .storage
        .get_editor(&checksum_address(editor_address2.to_string()), &space_id)
        .await;

    assert!(editor1.is_ok());
    assert!(editor2.is_ok());

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_removed_members() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the members table to ensure clean test state
    test_storage.clear_table("members").await?;

    let dao_address = generate_unique_address("remove_members_test_dao");
    let member_address = generate_unique_address("remove_members_test_mem");

    // First add a member
    let added_members = vec![make_added_member(&dao_address, &member_address)];
    let kg_data_add = make_kg_data_with_membership(1, added_members, vec![], vec![], vec![]);

    // Then remove the member
    let removed_members = vec![make_removed_member(&dao_address, &member_address)];
    let kg_data_remove = make_kg_data_with_membership(2, vec![], removed_members, vec![], vec![]);

    let blocks = vec![kg_data_add, kg_data_remove];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify member was removed
    let space_id = derive_space_id(GEO, &checksum_address(dao_address.to_string()));
    let member = indexer
        .storage
        .get_member(&checksum_address(member_address.to_string()), &space_id)
        .await;

    assert!(member.is_err()); // Should not exist after removal

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_removed_editors() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the editors table to ensure clean test state
    test_storage.clear_table("editors").await?;

    let dao_address = generate_unique_address("remove_editors_test_dao");
    let editor_address = generate_unique_address("remove_editors_test_edit");

    // First add an editor
    let added_editors = vec![make_added_member(&dao_address, &editor_address)];
    let kg_data_add = make_kg_data_with_membership(1, vec![], vec![], added_editors, vec![]);

    // Then remove the editor
    let removed_editors = vec![make_removed_member(&dao_address, &editor_address)];
    let kg_data_remove = make_kg_data_with_membership(2, vec![], vec![], vec![], removed_editors);

    let blocks = vec![kg_data_add, kg_data_remove];

    // Run the indexer
    indexer.run(&blocks).await?;

    // Verify editor was removed
    let space_id = derive_space_id(GEO, &checksum_address(dao_address.to_string()));
    let editor = indexer
        .storage
        .get_editor(&checksum_address(editor_address.to_string()), &space_id)
        .await;

    assert!(editor.is_err()); // Should not exist after removal

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_mixed_operations() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the membership tables to ensure clean test state
    test_storage.clear_table("members").await?;
    test_storage.clear_table("editors").await?;

    let dao_address = generate_unique_address("mixed_ops_test_dao");
    let member_address1 = generate_unique_address("mixed_ops_test_mem1");
    let member_address2 = generate_unique_address("mixed_ops_test_mem2");
    let editor_address1 = generate_unique_address("mixed_ops_test_edit1");
    let editor_address2 = generate_unique_address("mixed_ops_test_edit2");

    let added_members = vec![
        make_added_member(&dao_address, &member_address1),
        make_added_member(&dao_address, &member_address2),
    ];
    let removed_members = vec![
        make_removed_member(&dao_address, &member_address1), // Remove first member
    ];
    let added_editors = vec![
        make_added_member(&dao_address, &editor_address1),
        make_added_member(&dao_address, &editor_address2),
    ];
    let removed_editors = vec![
        make_removed_member(&dao_address, &editor_address1), // Remove first editor
    ];

    let kg_data = make_kg_data_with_membership(
        1,
        added_members,
        removed_members,
        added_editors,
        removed_editors,
    );
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    let space_id = derive_space_id(GEO, &checksum_address(dao_address.to_string()));

    // Verify only member2 exists
    let member1 = indexer
        .storage
        .get_member(&checksum_address(member_address1.to_string()), &space_id)
        .await;
    let member2 = indexer
        .storage
        .get_member(&checksum_address(member_address2.to_string()), &space_id)
        .await;
    assert!(member1.is_err()); // Should not exist
    assert!(member2.is_ok()); // Should exist

    // Verify only editor2 exists
    let editor1 = indexer
        .storage
        .get_editor(&checksum_address(editor_address1.to_string()), &space_id)
        .await;
    let editor2 = indexer
        .storage
        .get_editor(&checksum_address(editor_address2.to_string()), &space_id)
        .await;
    assert!(editor1.is_err()); // Should not exist
    assert!(editor2.is_ok()); // Should exist

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_multiple_spaces() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the membership tables to ensure clean test state
    test_storage.clear_table("members").await?;
    test_storage.clear_table("editors").await?;

    let dao_address1 = generate_unique_address("multi_spaces_test_dao1");
    let dao_address2 = generate_unique_address("multi_spaces_test_dao2");
    let member_address = generate_unique_address("multi_spaces_test_mem");
    let editor_address = generate_unique_address("multi_spaces_test_edit");

    let added_members = vec![
        make_added_member(&dao_address1, &member_address),
        make_added_member(&dao_address2, &member_address), // Same member in different spaces
    ];
    let added_editors = vec![
        make_added_member(&dao_address1, &editor_address),
        make_added_member(&dao_address2, &editor_address), // Same editor in different spaces
    ];

    let kg_data = make_kg_data_with_membership(1, added_members, vec![], added_editors, vec![]);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    let space_id1 = derive_space_id(GEO, &checksum_address(dao_address1.to_string()));
    let space_id2 = derive_space_id(GEO, &checksum_address(dao_address2.to_string()));

    // Verify member exists in both spaces
    let member1 = indexer
        .storage
        .get_member(&checksum_address(member_address.to_string()), &space_id1)
        .await;
    let member2 = indexer
        .storage
        .get_member(&checksum_address(member_address.to_string()), &space_id2)
        .await;
    assert!(member1.is_ok());
    assert!(member2.is_ok());

    // Verify editor exists in both spaces
    let editor1 = indexer
        .storage
        .get_editor(&checksum_address(editor_address.to_string()), &space_id1)
        .await;
    let editor2 = indexer
        .storage
        .get_editor(&checksum_address(editor_address.to_string()), &space_id2)
        .await;
    assert!(editor1.is_ok());
    assert!(editor2.is_ok());

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_membership_indexing_empty() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let _test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    let kg_data = make_kg_data_with_membership(1, vec![], vec![], vec![], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer - should not fail with empty membership data
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_space_indexing_duplicate_dao_addresses() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create test data with same DAO address for different space types
    let dao_address = generate_unique_address("duplicate_dao_test");
    let spaces = vec![
        make_personal_space(&dao_address),
        make_public_space(&dao_address),
    ];

    let kg_data = make_kg_data_with_spaces(5, vec![], spaces);
    let blocks = vec![kg_data];

    // Run the indexer - this should work since space IDs are derived differently
    // for personal vs public spaces (even with the same DAO address)
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_space_indexing_with_edits() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache);

    // Create some property operations
    let property_id = "1cc6995f-6cc2-4c7a-9592-1466bf95f6be";
    let property_op = make_property_op(property_id, PbDataType::String);

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
        cid: "".to_string(),
    };

    // Create spaces alongside edits
    let spaces = vec![
        make_personal_space(&generate_unique_address("space_with_edits_test_personal")),
        make_public_space(&generate_unique_address("space_with_edits_test_public")),
    ];

    let kg_data = make_kg_data_with_spaces(6, vec![item], spaces);
    let blocks = vec![kg_data];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

fn make_added_subspace(dao_address: &str, subspace_address: &str) -> AddedSubspace {
    AddedSubspace {
        dao_address: dao_address.to_string(),
        subspace_address: subspace_address.to_string(),
    }
}

fn make_removed_subspace(dao_address: &str, subspace_address: &str) -> RemovedSubspace {
    RemovedSubspace {
        dao_address: dao_address.to_string(),
        subspace_address: subspace_address.to_string(),
    }
}

fn make_kg_data_with_subspaces(
    block_number: u64,
    added_subspaces: Vec<AddedSubspace>,
    removed_subspaces: Vec<RemovedSubspace>,
) -> KgData {
    KgData {
        block: BlockMetadata {
            cursor: block_number.to_string(),
            block_number,
            timestamp: "1234567890".to_string(),
        },
        edits: vec![],
        spaces: vec![],
        added_members: vec![],
        removed_members: vec![],
        added_editors: vec![],
        removed_editors: vec![],
        added_subspaces,
        removed_subspaces,
    }
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_added_subspaces() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the subspaces and spaces tables to ensure clean test state
    test_storage.clear_table("subspaces").await?;
    test_storage.clear_table("spaces").await?;

    let parent_dao_address = generate_unique_address("add_subspaces_test_parent");
    let subspace_address1 = generate_unique_address("add_subspaces_test_sub1");
    let subspace_address2 = generate_unique_address("add_subspaces_test_sub2");

    // First create the spaces that will be referenced by the subspaces
    let spaces = vec![
        make_personal_space(&parent_dao_address),
        make_personal_space(&subspace_address1),
        make_personal_space(&subspace_address2),
    ];
    let kg_data_spaces = make_kg_data_with_spaces(1, vec![], spaces);

    // Then create the subspace relationships
    let added_subspaces = vec![
        make_added_subspace(&parent_dao_address, &subspace_address1),
        make_added_subspace(&parent_dao_address, &subspace_address2),
    ];
    let kg_data_subspaces = make_kg_data_with_subspaces(2, added_subspaces, vec![]);

    let blocks = vec![kg_data_spaces, kg_data_subspaces];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_removed_subspaces() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the subspaces and spaces tables to ensure clean test state
    test_storage.clear_table("subspaces").await?;
    test_storage.clear_table("spaces").await?;

    let parent_dao_address = generate_unique_address("remove_subspaces_test_parent");
    let subspace_address = generate_unique_address("remove_subspaces_test_sub");

    // First create the spaces
    let spaces = vec![
        make_personal_space(&parent_dao_address),
        make_personal_space(&subspace_address),
    ];
    let kg_data_spaces = make_kg_data_with_spaces(1, vec![], spaces);

    // Then add a subspace
    let added_subspaces = vec![make_added_subspace(&parent_dao_address, &subspace_address)];
    let kg_data_add = make_kg_data_with_subspaces(2, added_subspaces, vec![]);

    // Then remove the subspace
    let removed_subspaces = vec![make_removed_subspace(
        &parent_dao_address,
        &subspace_address,
    )];
    let kg_data_remove = make_kg_data_with_subspaces(3, vec![], removed_subspaces);

    let blocks = vec![kg_data_spaces, kg_data_add, kg_data_remove];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_mixed_operations() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the subspaces and spaces tables to ensure clean test state
    test_storage.clear_table("subspaces").await?;
    test_storage.clear_table("spaces").await?;

    let parent_dao_address = generate_unique_address("mixed_subspaces_test_parent");
    let subspace_address1 = generate_unique_address("mixed_subspaces_test_sub1");
    let subspace_address2 = generate_unique_address("mixed_subspaces_test_sub2");
    let subspace_address3 = generate_unique_address("mixed_subspaces_test_sub3");

    // First create the spaces
    let spaces = vec![
        make_personal_space(&parent_dao_address),
        make_personal_space(&subspace_address1),
        make_personal_space(&subspace_address2),
        make_personal_space(&subspace_address3),
    ];
    let kg_data_spaces = make_kg_data_with_spaces(1, vec![], spaces);

    // Then create subspace relationships with mixed operations
    let added_subspaces = vec![
        make_added_subspace(&parent_dao_address, &subspace_address1),
        make_added_subspace(&parent_dao_address, &subspace_address2),
        make_added_subspace(&parent_dao_address, &subspace_address3),
    ];
    let removed_subspaces = vec![
        make_removed_subspace(&parent_dao_address, &subspace_address1), // Remove first subspace
    ];
    let kg_data_subspaces = make_kg_data_with_subspaces(2, added_subspaces, removed_subspaces);

    let blocks = vec![kg_data_spaces, kg_data_subspaces];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_multiple_parents() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the subspaces and spaces tables to ensure clean test state
    test_storage.clear_table("subspaces").await?;
    test_storage.clear_table("spaces").await?;

    let parent_dao_address1 = generate_unique_address("multi_parents_test_parent1");
    let parent_dao_address2 = generate_unique_address("multi_parents_test_parent2");
    let subspace_address = generate_unique_address("multi_parents_test_sub");

    // First create the spaces
    let spaces = vec![
        make_personal_space(&parent_dao_address1),
        make_personal_space(&parent_dao_address2),
        make_personal_space(&subspace_address),
    ];
    let kg_data_spaces = make_kg_data_with_spaces(1, vec![], spaces);

    // Then create subspace relationships
    let added_subspaces = vec![
        make_added_subspace(&parent_dao_address1, &subspace_address),
        make_added_subspace(&parent_dao_address2, &subspace_address), // Same subspace in different parent spaces
    ];
    let kg_data_subspaces = make_kg_data_with_subspaces(2, added_subspaces, vec![]);

    let blocks = vec![kg_data_spaces, kg_data_subspaces];

    // Run the indexer
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_empty() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let _test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    let kg_data = make_kg_data_with_subspaces(1, vec![], vec![]);
    let blocks = vec![kg_data];

    // Run the indexer - should not fail with empty subspace data
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_subspace_indexing_with_other_operations() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let postgres_storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(postgres_storage.clone());
    let properties_cache = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(postgres_storage, properties_cache);

    // Clear the tables to ensure clean test state
    test_storage.clear_table("subspaces").await?;
    test_storage.clear_table("members").await?;
    test_storage.clear_table("spaces").await?;

    let dao_address = generate_unique_address("combined_ops_test_dao");
    let subspace_address = generate_unique_address("combined_ops_test_sub");
    let member_address = generate_unique_address("combined_ops_test_member");

    // Create test data with subspaces, members, and spaces combined
    let spaces = vec![
        make_personal_space(&dao_address),
        make_personal_space(&subspace_address), // Need to create child space too
    ];
    let added_subspaces = vec![make_added_subspace(&dao_address, &subspace_address)];
    let added_members = vec![make_added_member(&dao_address, &member_address)];

    let kg_data = KgData {
        block: BlockMetadata {
            cursor: "1".to_string(),
            block_number: 1,
            timestamp: "1234567890".to_string(),
        },
        edits: vec![],
        spaces,
        added_members,
        removed_members: vec![],
        added_editors: vec![],
        removed_editors: vec![],
        added_subspaces,
        removed_subspaces: vec![],
    };
    let blocks = vec![kg_data];

    // Run the indexer - should handle all operations together
    indexer.run(&blocks).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_properties_cache_initialization_from_database() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let test_storage = TestStorage::new(storage.clone());
    
    // Clear properties table to ensure clean test state
    test_storage.clear_table("properties").await?;
    
    // Define test properties with all data types
    let test_properties = vec![
        ("11111111-1111-1111-1111-111111111111", DataType::String),
        ("22222222-2222-2222-2222-222222222222", DataType::Number),
        ("33333333-3333-3333-3333-333333333333", DataType::Boolean),
        ("44444444-4444-4444-4444-444444444444", DataType::Time),
        ("55555555-5555-5555-5555-555555555555", DataType::Point),
        ("66666666-6666-6666-6666-666666666666", DataType::Relation),
    ];
    
    // Insert properties directly into database using the indexer
    let properties_cache_empty = Arc::new(PropertiesCache::new());
    let indexer = TestIndexer::new(storage.clone(), properties_cache_empty);
    
    // Create property operations for each test property
    let mut property_ops = Vec::new();
    for (property_id, data_type) in &test_properties {
        let pb_data_type = match data_type {
            DataType::String => PbDataType::String,
            DataType::Number => PbDataType::Number,
            DataType::Boolean => PbDataType::Boolean,
            DataType::Time => PbDataType::Time,
            DataType::Point => PbDataType::Point,
            DataType::Relation => PbDataType::Relation,
        };
        property_ops.push(make_property_op(property_id, pb_data_type));
    }
    
    // Create an edit with all property operations
    let edit = make_edit(
        "77777777-7777-7777-7777-777777777777",
        "Properties Cache Test Edit",
        "88888888-8888-8888-8888-888888888888",
        property_ops,
    );
    
    let item = PreprocessedEdit {
        edit: Some(edit),
        is_errored: false,
        space_id: Uuid::parse_str("99999999-9999-9999-9999-999999999999").unwrap(),
        cid: "".to_string(),
    };
    
    let kg_data = make_kg_data_with_spaces(1, vec![item], vec![]);
    let blocks = vec![kg_data];
    
    // Run the indexer to create properties in database
    indexer.run(&blocks).await?;
    
    // Verify properties were created in database
    for (property_id, expected_data_type) in &test_properties {
        let property = storage
            .get_property(&property_id.to_string())
            .await
            .unwrap();
        assert_eq!(property.data_type, *expected_data_type);
    }
    
    // Now test cache initialization from database
    let initialized_cache = PropertiesCache::from_storage(&storage).await
        .map_err(|e| IndexingError::StorageError(e))?;
    
    // Verify all properties are loaded into the cache
    for (property_id, expected_data_type) in &test_properties {
        let property_uuid = Uuid::parse_str(property_id).unwrap();
        let cached_data_type = initialized_cache.get(&property_uuid).await
            .map_err(|_| IndexingError::StorageError(StorageError::Database(sqlx::Error::RowNotFound)))?;
        assert_eq!(cached_data_type, *expected_data_type, 
                   "Property {} should have data type {:?} in cache", property_id, expected_data_type);
    }
    
    // Test cache behavior: accessing non-existent property should return error
    let non_existent_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
    let result = initialized_cache.get(&non_existent_id).await;
    assert!(result.is_err(), "Non-existent property should return error");
    
    // Test empty database scenario
    test_storage.clear_table("properties").await?;
    let empty_cache = PropertiesCache::from_storage(&storage).await
        .map_err(|e| IndexingError::StorageError(e))?;
    
    // Any property lookup should fail on empty cache
    let result = empty_cache.get(&test_properties[0].0.parse().unwrap()).await;
    assert!(result.is_err(), "Empty cache should return error for any property");
    
    Ok(())
}
