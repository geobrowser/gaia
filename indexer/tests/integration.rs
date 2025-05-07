use std::sync::Arc;
use thiserror::Error;

use dotenv::dotenv;
use indexer::{
    cache::{Cache, CacheError},
    storage::{postgres::PostgresStorage, StorageError},
};
use prost::DecodeError;

#[derive(Error, Debug)]
pub enum IndexingError {
    #[error("Indexing error: {0}")]
    StorageError(#[from] StorageError),

    #[error("Indexing error: {0}")]
    CacheError(#[from] CacheError),

    #[error("Indexing error: {0}")]
    DecodeError(#[from] DecodeError),
}

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

#[tokio::test]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let storage = PostgresStorage::new().await;

    match storage {
        Ok(result) => {
            let cache = Cache::new().await?;
            let indexer = TestIndexer::new(result, cache);
        }
        Err(error) => {
            println!("Error initializing stream {}", error);
        }
    }

    Ok(())
}
