use futures::future::join_all;
use indexer::{
    cache::{Cache, CacheError},
    storage::{
        entities::EntitiesModel, postgres::PostgresStorage, storage::StorageBackend,
        storage::StorageError,
    },
};
use std::{env, sync::Arc};
use thiserror::Error;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use dotenv::dotenv;
use prost::{DecodeError, Message};
use stream::{utils::BlockMetadata, Sink};
use tokio::task;

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

use grc20::pb::chain::GeoOutput;

#[derive(Error, Debug)]
pub enum IndexingError {
    #[error("Indexing error: {0}")]
    StorageError(#[from] StorageError),

    #[error("Indexing error: {0}")]
    CacheError(#[from] CacheError),

    #[error("Indexing error: {0}")]
    DecodeError(#[from] DecodeError),
}

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
        process_block(block_data, &self.storage, &self.cache).await?;

        Ok(())
    }
}

async fn process_block(
    // @TODO: What the minimum data we need from the block?
    block_data: &stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    storage: &Arc<PostgresStorage>,
    cache: &Arc<Cache>,
) -> Result<(), IndexingError> {
    let output = stream::utils::output(block_data);
    let geo = GeoOutput::decode(output.value.as_slice())?;
    let block_metadata = stream::utils::block_metadata(block_data);

    println!(
        "Block #{} - Payload {} ({} bytes) - Drift {}s – Edits Published {}",
        block_metadata.block_number,
        output.type_url.replace("type.googleapis.com/", ""),
        output.value.len(),
        block_metadata.timestamp,
        geo.edits_published.len()
    );

    let mut handles = Vec::new();

    for edit in geo.edits_published {
        let storage = storage.clone();
        let cache = cache.clone();
        let block_metadata = stream::utils::block_metadata(block_data);

        let handle = task::spawn(async move {
            // We retry requests to the cache in the case that the cache is
            // still populating. For now we assume writing to + reading from
            // the cache can't fail
            let retry = ExponentialBackoff::from_millis(10)
                .factor(2)
                .max_delay(std::time::Duration::from_secs(5))
                .map(jitter);
            let edit = Retry::spawn(retry, async || cache.get(&edit.content_uri).await).await;

            match edit {
                Ok(value) => {
                    if !value.is_errored {
                        let entities = EntitiesModel::map_edit_to_entities(
                            &value.edit.unwrap(),
                            &block_metadata,
                        );
                        let result = storage.insert_entities(&entities).await;

                        match result {
                            Ok(value) => {}
                            Err(error) => {
                                println!("Error writing {}", error);
                            }
                        }
                    }
                }
                Err(error) => {
                    //
                }
            }

            Ok::<(), IndexingError>(())
        });

        handles.push(handle);
    }

    // Wait for all processing in the current block to finish before continuing
    // to the next block
    let done = join_all(handles).await;

    Ok(())
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
