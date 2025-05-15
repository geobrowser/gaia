use grc20::pb::ipfsv2::{Edit, Entity, Op, Value};
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
                    OpType::SET,
                    "entity-id-1",
                    vec![TestValue {
                        property_id: "attribute-id".to_string(),
                        value: Some("value 1".to_string()),
                    }],
                ),
                make_entity_op(
                    OpType::SET,
                    "entity-id-2",
                    vec![TestValue {
                        property_id: "attribute-id".to_string(),
                        value: Some("value 2".to_string()),
                    }],
                ),
                make_entity_op(
                    OpType::DELETE,
                    "entity-id-2",
                    vec![TestValue {
                        property_id: "attribute-id".to_string(),
                        value: None,
                    }],
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
        let triple = storage
            .get_property(&"entity-id-1:attribute-id:5".to_string())
            .await
            .unwrap();
        assert_eq!(triple.id, "entity-id-1:attribute-id:5");
    }

    {
        let triple = storage
            .get_property(&"entity-id-2:attribute-id:5".to_string())
            .await
            .unwrap();

        // @TODO: SHould not exist
        assert_eq!(triple.id, "entity-id-2:attribute-id:5");
    }

    Ok(())
}

fn make_edit(id: &str, name: &str, author: &str, ops: Vec<Op>) -> Edit {
    Edit {
        id: String::from(id),
        name: String::from(name),
        version: String::from("0.0.1"),
        ops,
        r#type: 1,
        authors: vec![String::from(author)],
    }
}

enum OpType {
    SET,
    DELETE,
}

struct TestValue {
    pub property_id: String,
    pub value: Option<String>,
}

fn make_entity_op(op_type: OpType, entity: &str, values: Vec<TestValue>) -> Op {
    match op_type {
        OpType::SET => Op {
            r#type: 2, // Update entity
            entity: Some(Entity {
                id: entity.to_string().into_bytes(),
                values: values
                    .iter()
                    .map(|v| Value {
                        options: None,
                        property_id: v.property_id.clone().into_bytes(),
                        value: Some(v.value.clone().unwrap()),
                    })
                    .collect(),
            }),
            relation: None,
            property: None,
        },
        OpType::DELETE => Op {
            r#type: 7, // Unset properties
            entity: Some(Entity {
                id: entity.to_string().into_bytes(),
                values: values
                    .iter()
                    .map(|v| Value {
                        options: None,
                        property_id: v.property_id.clone().into_bytes(),
                        value: None,
                    })
                    .collect(),
            }),
            relation: None,
            property: None,
        },
    }
}
