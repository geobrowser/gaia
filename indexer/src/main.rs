use grc20::pb::chain::GeoOutput;
use indexer::{
    block_handler::root_handler, cache::postgres::PostgresCache, error::IndexingError,
    storage::postgres::PostgresStorage,
};
use prost::Message;
use std::{env, sync::Arc};

use dotenv::dotenv;
use stream::{pb::sf::substreams::rpc::v2::BlockScopedData, utils::BlockMetadata, Sink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

struct KgData {
    pub block: BlockMetadata,
}

struct KgIndexer {
    storage: Arc<PostgresStorage>,
    cache: Arc<PostgresCache>,
}

impl KgIndexer {
    pub fn new(storage: PostgresStorage, cache: PostgresCache) -> Self {
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
        block_data: &BlockScopedData,
    ) -> Result<(), Self::Error> {
        let output = stream::utils::output(block_data);
        let block_metadata = stream::utils::block_metadata(block_data);
        let geo = GeoOutput::decode(output.value.as_slice())?;

        // @TODO: Need to figure out to abstract the different types of streams so
        // people can write their own sinks over specific events however they want.
        //
        // One idea is implementing the decoding at the stream level, so anybody
        // consuming the stream just gets the block data + the already-decoded contents
        // of each event.
        //
        // async fn process_block(&self, block_data: &DecodedBlockData, _raw_block_data: &BlockScopedData);
        root_handler::run(&geo, &block_metadata, &self.storage, &self.cache).await?;

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
            let cache = PostgresCache::new().await?;
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
