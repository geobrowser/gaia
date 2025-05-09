use grc20::pb::{
    chain::{EditPublished, GeoOutput},
    ipfs::{Edit, Op, Triple, Value},
};
use std::{env, sync::Arc};
use stream::utils::BlockMetadata;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{
        kv::{KvCache, WriteCacheItem},
        PreprocessedEdit,
    },
    error::IndexingError,
    storage::postgres::PostgresStorage,
};

struct TestIndexer {
    storage: Arc<PostgresStorage>,
    cache: Arc<KvCache>,
}

impl TestIndexer {
    pub fn new(storage: Arc<PostgresStorage>, cache: Arc<KvCache>) -> Self {
        TestIndexer { storage, cache }
    }

    pub async fn run(&self, blocks: &Vec<TestBlock>) -> Result<(), IndexingError> {
        for block in blocks {
            root_handler::run(&block.output, &block.block, &self.storage, &self.cache).await?;
        }

        Ok(())
    }
}

struct TestBlock {
    output: GeoOutput,
    block: BlockMetadata,
}

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);

    let test_output_1 = TestBlock {
        output: GeoOutput {
            spaces_created: Vec::new(),
            governance_plugins_created: Vec::new(),
            initial_editors_added: Vec::new(),
            votes_cast: Vec::new(),
            edits_published: vec![EditPublished {
                content_uri: String::from("5"),
                dao_address: String::from("5"),
                plugin_address: String::from("5"),
            }],
            successor_spaces_created: Vec::new(),
            subspaces_added: Vec::new(),
            subspaces_removed: Vec::new(),
            executed_proposals: Vec::new(),
            members_added: Vec::new(),
            editors_added: Vec::new(),
            personal_plugins_created: Vec::new(),
            members_removed: Vec::new(),
            editors_removed: Vec::new(),
            edits: Vec::new(),
            proposed_added_members: Vec::new(),
            proposed_removed_members: Vec::new(),
            proposed_added_editors: Vec::new(),
            proposed_removed_editors: Vec::new(),
            proposed_added_subspaces: Vec::new(),
            proposed_removed_subspaces: Vec::new(),
        },
        block: {
            BlockMetadata {
                cursor: String::from("5"),
                block_number: 1,
                timestamp: String::from("5"),
            }
        },
    };

    let cache = Arc::new(
        KvCache::new(vec![WriteCacheItem {
            uri: String::from("5"),
            item: PreprocessedEdit {
                space_id: String::from("5"),
                edit: Some(make_edit(
                    "5",
                    "Name",
                    "Author",
                    vec![
                        make_triple_op(OpType::SET, "entity-id-1", "attribute-id", "value 1", 1),
                        make_triple_op(OpType::SET, "entity-id-2", "attribute-id", "value 2", 1),
                        make_triple_op(OpType::DELETE, "entity-id-2", "attribute-id", "value 2", 1),
                    ],
                )),
                is_errored: false,
            },
        }])
        .await?,
    );

    let indexer = TestIndexer::new(storage.clone(), cache);
    indexer.run(&vec![test_output_1]).await?;

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
            .get_triple(&"entity-id-1:attribute-id:5".to_string())
            .await
            .unwrap();
        assert_eq!(triple.id, "entity-id-1:attribute-id:5");
    }

    {
        let triple = storage
            .get_triple(&"entity-id-2:attribute-id:5".to_string())
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

fn make_triple_op(
    op_type: OpType,
    entity: &str,
    attribute: &str,
    value: &str,
    value_type: i32,
) -> Op {
    match op_type {
        OpType::SET => Op {
            r#type: 1,
            entity: None,
            triples: vec![],
            metadata: None,
            relation: None,
            url: None,
            triple: Some(Triple {
                attribute: attribute.to_string(),
                entity: entity.to_string(),
                value: Some(Value {
                    options: None,
                    r#type: value_type,
                    value: value.to_string(),
                }),
            }),
        },
        OpType::DELETE => Op {
            r#type: 2,
            entity: None,
            triples: vec![],
            metadata: None,
            relation: None,
            url: None,
            triple: Some(Triple {
                attribute: attribute.to_string(),
                entity: entity.to_string(),
                value: Some(Value {
                    options: None,
                    r#type: value_type,
                    value: value.to_string(),
                }),
            }),
        },
    }
}
