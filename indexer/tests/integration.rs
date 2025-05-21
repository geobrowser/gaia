use grc20::pb::ipfsv2::{op::Payload, Edit, Entity, Op, Relation, UnsetProperties, Value};
use std::{env, sync::Arc};
use stream::utils::BlockMetadata;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler, cache::PreprocessedEdit, error::IndexingError,
    storage::postgres::PostgresStorage, KgData,
};

struct TestIndexer {
    storage: Arc<PostgresStorage>,
}

impl TestIndexer {
    pub fn new(storage: Arc<PostgresStorage>) -> Self {
        TestIndexer { storage }
    }

    pub async fn run(&self, blocks: &Vec<KgData>) -> Result<(), IndexingError> {
        for block in blocks {
            root_handler::run(block.edits.clone(), &block.block, &self.storage).await?;
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
        space_id: String::from("5"),
        edit: Some(make_edit(
            "5",
            "Name",
            "Author",
            vec![
                make_entity_op(
                    TestEntityOpType::CREATE,
                    "entity-id-1",
                    vec![
                        TestValue {
                            property_id: "LuBWqZAu6pz54eiJS5mLv8".to_string(),
                            value: Some("Test entity".to_string()),
                        },
                        TestValue {
                            property_id: "attribute-id".to_string(),
                            value: Some("value 1".to_string()),
                        },
                    ],
                ),
                make_entity_op(
                    TestEntityOpType::UPDATE,
                    "entity-id-2",
                    vec![TestValue {
                        property_id: "attribute-id".to_string(),
                        value: Some("value 2".to_string()),
                    }],
                ),
                make_entity_op(
                    TestEntityOpType::UNSET,
                    "entity-id-2",
                    vec![TestValue {
                        property_id: "attribute-id".to_string(),
                        value: None,
                    }],
                ),
                make_relation_op(
                    TestRelationOpType::CREATE,
                    "relation-id-1",
                    "entity-id-1",
                    "type-id-1",
                    "from-entity-1",
                    "to-entity-1",
                ),
                make_relation_op(
                    TestRelationOpType::UPDATE,
                    "relation-id-1",
                    "entity-id-1",
                    "type-id-1",
                    "from-entity-1",
                    "to-entity-1",
                ),
                make_relation_op(
                    TestRelationOpType::CREATE,
                    "relation-id-2",
                    "entity-id-1",
                    "type-id-1",
                    "from-entity-1",
                    "to-entity-1",
                ),
                make_relation_op(
                    TestRelationOpType::DELETE,
                    "relation-id-2",
                    "entity-id-1",
                    "type-id-1",
                    "from-entity-1",
                    "to-entity-1",
                ),
            ],
        )),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("5"),
        block_number: 1,
        timestamp: String::from("5"),
    };

    let indexer = TestIndexer::new(storage.clone());

    indexer
        .run(&vec![KgData {
            block,
            edits: vec![item],
        }])
        .await?;

    {
        let entity = storage
            .get_entity(&"entity-id-1".to_string())
            .await
            .unwrap();
        assert_eq!(entity.id, "entity-id-1");
    }

    {
        let entity = storage
            .get_entity(&"entity-id-2".to_string())
            .await
            .unwrap();
        assert_eq!(entity.id, "entity-id-2");
    }

    {
        let attribute = storage
            .get_entity(&"attribute-id".to_string())
            .await
            .unwrap();
        assert_eq!(attribute.id, "attribute-id");
    }

    {
        let value = storage
            .get_value(&"entity-id-1:attribute-id:5".to_string())
            .await
            .unwrap();
        assert_eq!(value.id, "entity-id-1:attribute-id:5");
    }

    {
        let value = storage
            .get_value(&"entity-id-2:attribute-id:5".to_string())
            .await;

        // Should not return the value since it was deleted
        assert_eq!(value.is_err(), true);
    }

    {
        let value = storage
            .get_relation(&"relation-id-1".to_string())
            .await
            .unwrap();

        assert_eq!(value.id, "relation-id-1");
        assert_eq!(value.entity_id, "entity-id-1");
        assert_eq!(value.from_id, "from-entity-1");
        assert_eq!(value.to_id, "to-entity-1");
        assert_eq!(value.space_id, "5");

        // Update in edit sets verified to Some(true)
        assert_eq!(value.verified, Some(true));
    }

    {
        // Should not return the value since it was deleted
        let value = storage.get_relation(&"relation-id-2".to_string()).await;
        assert_eq!(value.is_err(), true);
    }

    Ok(())
}

fn make_edit(id: &str, name: &str, author: &str, ops: Vec<Op>) -> Edit {
    Edit {
        id: String::from(id).into_bytes(),
        name: String::from(name),
        ops,
        authors: vec![String::from(author).into_bytes()],
        language: None,
    }
}

struct TestValue {
    pub property_id: String,
    pub value: Option<String>,
}

enum TestEntityOpType {
    CREATE,
    UPDATE,
    UNSET,
}

fn make_entity_op(op_type: TestEntityOpType, entity: &str, values: Vec<TestValue>) -> Op {
    match op_type {
        TestEntityOpType::CREATE => Op {
            payload: Some(Payload::CreateEntity(Entity {
                id: entity.to_string().into_bytes(),
                values: values
                    .iter()
                    .map(|v| Value {
                        property_id: v.property_id.clone().into_bytes(),
                        value: v.value.clone().unwrap(),
                    })
                    .collect(),
            })),
        },
        TestEntityOpType::UPDATE => Op {
            payload: Some(Payload::UpdateEntity(Entity {
                id: entity.to_string().into_bytes(),
                values: values
                    .iter()
                    .map(|v| Value {
                        property_id: v.property_id.clone().into_bytes(),
                        value: v.value.clone().unwrap(),
                    })
                    .collect(),
            })),
        },
        TestEntityOpType::UNSET => Op {
            payload: Some(Payload::UnsetProperties(UnsetProperties {
                id: entity.to_string().into_bytes(),
                properties: values
                    .iter()
                    .map(|v| v.property_id.clone().into_bytes())
                    .collect(),
            })),
        },
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
                id: relation_id.to_string().into_bytes(),
                r#type: type_id.to_string().into_bytes(),
                entity: entity_id.to_string().into_bytes(),
                from_entity: from_entity.to_string().into_bytes(),
                from_space: None,
                from_version: None,
                to_entity: to_entity.to_string().into_bytes(),
                to_space: None,
                to_version: None,
                position: None,
                verified: None,
            })),
        },
        TestRelationOpType::UPDATE => Op {
            payload: Some(Payload::UpdateRelation(grc20::pb::ipfsv2::RelationUpdate {
                relation_id: relation_id.to_string().into_bytes(),
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
                relation_id.to_string().into_bytes(),
            )),
        },
    }
}
