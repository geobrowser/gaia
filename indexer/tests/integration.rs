use std::sync::Arc;

use dotenv::dotenv;
use indexer::{cache::Cache, storage::postgres::PostgresStorage};

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
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let storage = PostgresStorage::new().await;

    match storage {
        Ok(result) => {
            let cache = Cache::new().await?;
            let indexer = KgIndexer::new(result, cache);

            let endpoint_url =
                env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");

            let _result = indexer
                .run(&endpoint_url, PKG_FILE, MODULE_NAME, START_BLOCK, 0)
                .await;
        }
        Err(error) => {
            println!("Error initializing stream {}", error);
        }
    }

    Ok(())
}
