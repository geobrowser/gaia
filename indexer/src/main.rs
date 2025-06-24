use indexer::{
    block_handler::root_handler,
    cache::{postgres::PostgresCache, properties_cache::PropertiesCache},
    error::IndexingError,
    preprocess,
    storage::postgres::PostgresStorage,
    KgData,
};
use std::{env, sync::Arc};

use dotenv::dotenv;
use stream::{pb::sf::substreams::rpc::v2::BlockScopedData, PreprocessedSink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 54900;

struct KgIndexer {
    storage: Arc<PostgresStorage>,
    ipfs_cache: Arc<PostgresCache>,
    properties_cache: Arc<PropertiesCache>,
}

impl KgIndexer {
    pub fn new(
        storage: PostgresStorage,
        ipfs_cache: PostgresCache,
        properties_cache: PropertiesCache,
    ) -> Self {
        KgIndexer {
            storage: Arc::new(storage),
            ipfs_cache: Arc::new(ipfs_cache),
            properties_cache: Arc::new(properties_cache),
        }
    }
}

impl PreprocessedSink<KgData> for KgIndexer {
    type Error = IndexingError;

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        Ok(Some("".to_string()))
    }

    async fn persist_cursor(&self, _cursor: String) -> Result<(), Self::Error> {
        Ok(())
    }

    /**
    We can pre-process any edits we care about in the chain in this separate function.
    There's lots of decoding steps and filtering done to the Knowledge Graphs events
    so it's helpful to do this decoding/filtering/data-fetching ahead of time so the
    process steps can focus purely on mapping and writing data to the sink.
    */
    async fn preprocess_block_scoped_data(
        &self,
        block_data: &BlockScopedData,
    ) -> Result<KgData, Self::Error> {
        let kg_data =
            preprocess::preprocess_block_scoped_data(block_data, &self.ipfs_cache).await?;

        Ok(kg_data)
    }

    async fn process_block_scoped_data(
        &self,
        _block_data: &BlockScopedData,
        decoded_data: KgData,
    ) -> Result<(), Self::Error> {
        // @TODO: Need to figure out to abstract the different types of streams so
        // people can write their own sinks over specific events however they want.
        //
        // One idea is implementing the decoding at the stream level, so anybody
        // consuming the stream just gets the block data + the already-decoded contents
        // of each event.
        //
        // async fn process_block(&self, block_data: &DecodedBlockData, _raw_block_data: &BlockScopedData);
        root_handler::run(
            &decoded_data,
            &decoded_data.block,
            &self.storage,
            &self.properties_cache,
        )
        .await?;

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
            let properties_cache = PropertiesCache::new();
            let indexer = KgIndexer::new(result, cache, properties_cache);

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
