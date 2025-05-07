use grc20::pb::{
    chain::{EditPublished, GeoOutput},
    ipfs::{Edit, Op, Triple, Value},
};
use std::sync::Arc;
use stream::utils::BlockMetadata;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{
        kv::{KvCache, WriteCacheItem},
        CacheItem,
    },
    error::IndexingError,
    storage::kv::KvStorage,
};

struct TestIndexer {
    storage: Arc<KvStorage>, // @TODO: Can use in-memory?
    cache: Arc<KvCache>,     // @TODO: Can use in-memory
}

impl TestIndexer {
    pub fn new(storage: Arc<KvStorage>, cache: Arc<KvCache>) -> Self {
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
    let storage = Arc::new(KvStorage::new());

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
            item: CacheItem {
                edit: Some(Edit {
                    id: String::from("1"),
                    name: String::from("test"),
                    version: String::from("0.0.1"),
                    ops: vec![Op {
                        r#type: 1,
                        entity: None,
                        triples: vec![],
                        metadata: None,
                        relation: None,
                        url: None,
                        triple: Some(Triple {
                            attribute: "attribute".to_string(),
                            entity: "entity".to_string(),
                            value: Some(Value {
                                options: None,
                                r#type: 1,
                                value: "value".to_string(),
                            }),
                        }),
                    }],
                    r#type: 1,
                    authors: vec![String::from("Byron")],
                }),
                is_errored: false,
            },
        }])
        .await?,
    );

    let indexer = TestIndexer::new(storage.clone(), cache);
    indexer.run(&vec![test_output_1]).await?;

    let result = storage.clone().get(&"entity".to_string()).await.unwrap();
    println!("result {}", result.created_at);

    Ok(())
}
