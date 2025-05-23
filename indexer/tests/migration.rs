use grc20::pb::ipfsv2::Op;
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

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    // dotenv().ok();
    // let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    // let storage = Arc::new(PostgresStorage::new(&database_url).await?);

    // let file = include_str!("./25omwWh6HYgeRQKCaSpVpa_ops.json");
    // let ops_from_file: Vec<Op> = serde_json::from_str(file).unwrap();

    // println!("ops_from_file {:?}", ops_from_file);

    // let item = PreprocessedEdit {
    //     space_id: String::from("5"),
    //     edit: None,
    //     is_errored: true,
    // };

    // let block = BlockMetadata {
    //     cursor: String::from("5"),
    //     block_number: 1,
    //     timestamp: String::from("5"),
    // };

    // let indexer = TestIndexer::new(storage.clone());

    // indexer
    //     .run(&vec![KgData {
    //         block,
    //         edits: vec![item],
    //     }])
    //     .await?;

    Ok(())
}
