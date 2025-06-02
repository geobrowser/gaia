use bytes::Bytes;
use grc20::pb::grc20::Edit;
use prost::Message;
use std::{env, fs, sync::Arc};
use stream::utils::BlockMetadata;

use dotenv::dotenv;
use indexer::{
    block_handler::root_handler,
    cache::{properties_cache::PropertiesCache, PreprocessedEdit},
    error::IndexingError,
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

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let properties_cache = Arc::new(PropertiesCache::new());

    let root_space_bytes = fs::read("./tests/25omwWh6HYgeRQKCaSpVpa_ops");
    let crypto_space_bytes = fs::read("./tests/SgjATMbm41LX6naizMqBVd_ops");

    let root_space_edit = Edit::decode(Bytes::from(root_space_bytes.unwrap()));
    let crypto_space_edit = Edit::decode(Bytes::from(crypto_space_bytes.unwrap()));

    let root_space_preprocessed_edit = PreprocessedEdit {
        space_id: String::from("25omwWh6HYgeRQKCaSpVpa"),
        edit: Some(root_space_edit.clone().unwrap()),
        is_errored: false,
    };

    let crypto_space_preprocessed_edit = PreprocessedEdit {
        space_id: String::from("SgjATMbm41LX6naizMqBVd_ops"),
        edit: Some(crypto_space_edit.clone().unwrap()),
        is_errored: false,
    };

    let block = BlockMetadata {
        cursor: String::from("5"),
        block_number: 1,
        timestamp: String::from("5"),
    };

    let indexer = TestIndexer::new(storage.clone(), properties_cache.clone());

    indexer
        .run(&vec![KgData {
            block,
            edits: vec![root_space_preprocessed_edit, crypto_space_preprocessed_edit],
        }])
        .await?;

    Ok(())
}
