use grc20::pb::chain::GeoOutput;
use std::{env, sync::Arc};
use stream::pb::sf::substreams::{
    rpc::v2::{BlockScopedData, MapModuleOutput},
    v1::Clock,
};

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler, cache::Cache, error::IndexingError,
    storage::postgres::PostgresStorage,
};

struct TestIndexer {
    storage: Arc<PostgresStorage>, // @TODO: Can use in-memory?
    cache: Arc<Cache>,             // @TODO: Can use in-memory
}

impl TestIndexer {
    pub fn new(storage: PostgresStorage, cache: Cache) -> Self {
        TestIndexer {
            storage: Arc::new(storage),
            cache: Arc::new(cache),
        }
    }

    pub async fn run(&self, blocks: &Vec<BlockScopedData>) -> Result<(), IndexingError> {
        for block in blocks {
            let block_metadata = stream::utils::block_metadata(block);

            root_handler::run(
                &GeoOutput {
                    spaces_created: Vec::new(),
                    governance_plugins_created: Vec::new(),
                    initial_editors_added: Vec::new(),
                    votes_cast: Vec::new(),
                    edits_published: Vec::new(),
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
                &block_metadata,
                &self.storage,
                &self.cache,
            )
            .await?;
        }

        Ok(())
    }
}

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = PostgresStorage::new(&database_url).await;

    match storage {
        Ok(result) => {
            let cache = Cache::new().await?;
            let indexer = TestIndexer::new(result, cache);

            let block = BlockScopedData::make(MapModuleOutput {
                name: String::from("Test"),
                map_output: (),
                debug_info: (),
            });

            indexer.run(&Vec::new()).await?;
        }
        Err(error) => {
            println!("Error initializing stream {}", error);
        }
    }

    Ok(())
}

trait Make {
    fn make(output: &MapModuleOutput, clock: &Clock) -> Self;
}

impl Make for BlockScopedData {
    fn make(output: &MapModuleOutput, clock: &Clock) -> Self {
        BlockScopedData {
            output: Some(output.clone()),
            clock: Some(clock.clone()),
            cursor: String::from(""),
            final_block_height: 5,
            debug_map_outputs: Vec::new(),
            debug_store_outputs: Vec::new(),
        }
    }
}
