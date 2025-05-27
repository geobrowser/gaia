use bytes::Bytes;
use grc20::pb::ipfs::Edit;
use prost::Message;
use std::{env, fs, sync::Arc};
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

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = Arc::new(PostgresStorage::new(&database_url).await?);
    let bytes = fs::read("./tests/25omwWh6HYgeRQKCaSpVpa_ops");

    let edit = Edit::decode(Bytes::from(bytes.unwrap()));

    println!(
        "Running migration tests for edit {:?}",
        edit.clone().unwrap().name
    );

    let item = PreprocessedEdit {
        space_id: String::from("5"),
        edit: Some(edit.clone().unwrap()),
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

    Ok(())
}
