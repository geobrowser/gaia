use futures::future::join_all;
use grc20::pb::chain::GeoOutput;
use indexer::{
    block_handler::root_handler,
    cache::{postgres::PostgresCache, CacheBackend, PreprocessedEdit},
    error::IndexingError,
    storage::postgres::PostgresStorage,
    KgData,
};
use indexer_utils::get_blocklist;
use prost::Message;
use std::{env, sync::Arc};
use tokio::{sync::Mutex, task};
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use dotenv::dotenv;
use stream::{pb::sf::substreams::rpc::v2::BlockScopedData, PreprocessedSink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

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
    ) -> Result<(), Self::Error> {
        let output = stream::utils::output(block_data);
        let block_metadata = stream::utils::block_metadata(block_data);
        let geo = GeoOutput::decode(output.value.as_slice())?;
        let cache = &self.cache;
        let edits = Arc::new(Mutex::new(Vec::<PreprocessedEdit>::new()));

        let mut handles = Vec::new();

        // @TODO: We can separate this cache reading step into a separate module
        for chain_edit in geo.edits_published.clone() {
            if get_blocklist()
                .dao_addresses
                .contains(&chain_edit.dao_address.as_str())
            {
                continue;
            }

            let cache = cache.clone();
            let edits_clone = edits.clone();

            let handle = task::spawn(async move {
                // We retry requests to the cache in the case that the cache is
                // still populating. For now we assume writing to + reading from
                // the cache can't fail
                let retry = ExponentialBackoff::from_millis(10)
                    .factor(2)
                    .max_delay(std::time::Duration::from_secs(5))
                    .map(jitter);
                let cached_edit_entry =
                    Retry::spawn(retry, async || cache.get(&chain_edit.content_uri).await).await?;

                {
                    let mut edits_guard = edits_clone.lock().await;
                    edits_guard.push(cached_edit_entry);
                }

                Ok::<(), IndexingError>(())
            });

            handles.push(handle);
        }

        join_all(handles).await;

        // Extract the edits from the Arc<Mutex<>> for further processing
        let final_edits = {
            let edits_guard = edits.lock().await;
            edits_guard.clone() // Clone the vector to move it out of the mutex
        };

        self.process_block_scoped_data(
            block_data,
            KgData {
                edits: final_edits,
                block: block_metadata,
            },
        )
        .await?;

        Ok(())
    }

    async fn process_block_scoped_data(
        &self,
        _block_data: &BlockScopedData,
        decoded_data: KgData,
    ) -> Result<(), Self::Error> {
        let block_metadata = decoded_data.block;

        // @TODO: Need to figure out to abstract the different types of streams so
        // people can write their own sinks over specific events however they want.
        //
        // One idea is implementing the decoding at the stream level, so anybody
        // consuming the stream just gets the block data + the already-decoded contents
        // of each event.
        //
        // async fn process_block(&self, block_data: &DecodedBlockData, _raw_block_data: &BlockScopedData);
        root_handler::run(decoded_data.edits, &block_metadata, &self.storage).await?;

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
