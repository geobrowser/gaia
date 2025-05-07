use indexer::{
    block_handler::root_handler,
    cache::Cache,
    error::IndexingError,
    storage::{postgres::PostgresStorage, StorageBackend},
};
use std::{env, sync::Arc};

use dotenv::dotenv;
use stream::{utils::BlockMetadata, Sink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

struct KgData {
    pub block: BlockMetadata,
}

struct KgIndexer {
    storage: Arc<PostgresStorage>,
    cache: Arc<Cache>,
}

impl KgIndexer {
    pub fn new(storage: PostgresStorage, cache: Cache) -> Self {
        KgIndexer {
            storage: Arc::new(storage),
            cache: Arc::new(cache),
        }
    }
}

impl Sink<KgData> for KgIndexer {
    type Error = IndexingError;

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        Ok(Some("".to_string()))
    }

    async fn persist_cursor(&self, _cursor: String) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn process_block_scoped_data(
        &self,
        block_data: &stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    ) -> Result<(), Self::Error> {
        // @TODO: Need to figure out how to abstract the below into a unique function
        // so we can write testable mechanisms for the stream handling itself. i.e.,
        // how do we mock the stream?
        //
        // @TODO: Need to figure out to abstract the different types of streams so
        // people can write their own sinks over specific events however they want.
        root_handler::run(block_data, &self.storage, &self.cache).await?;

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let storage = PostgresStorage::new(&database_url).await;

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
