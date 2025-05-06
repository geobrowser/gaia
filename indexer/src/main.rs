use futures::future::join_all;
use std::{env, sync::Arc};
use thiserror::Error;
use tokio_retry::{
    strategy::{jitter, ExponentialBackoff},
    Retry,
};

use dotenv::dotenv;
use prost::{DecodeError, Message};
use stream::{utils::BlockMetadata, Sink};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

use grc20::pb::chain::GeoOutput;

mod storage;
use storage::{EntityStorage, EntityStorageError, Storage};
mod cache;
use cache::{Cache, CacheError};
use tokio::task;

#[derive(Error, Debug)]
pub enum IndexingError {
    #[error("Indexing error: {0}")]
    EntityStorageError(#[from] EntityStorageError),

    #[error("Indexing error: {0}")]
    CacheError(#[from] CacheError),

    #[error("Indexing error: {0}")]
    DecodeError(#[from] DecodeError),
}

struct KgData {
    pub block: BlockMetadata,
}

struct KgIndexer {
    entity_storage: Arc<EntityStorage>,
    cache: Arc<Cache>,
}

impl KgIndexer {
    pub fn new(entity_storage: EntityStorage, cache: Cache) -> Self {
        KgIndexer {
            entity_storage: Arc::new(entity_storage),
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
            let storage = self.entity_storage.clone();
            let cache = self.cache.clone();
            let block_metadata = stream::utils::block_metadata(block_data);

            // 18m 52s cache processor
            // ~31m    edits

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
                            let entities = storage
                                .map_edit_to_entity_items(value.edit.unwrap(), &block_metadata);
                            let result = storage.insert(&entities).await;

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
}

#[tokio::main]
async fn main() -> Result<(), IndexingError> {
    dotenv().ok();

    let storage = Storage::new().await;

    match storage {
        Ok(result) => {
            let entity_storage = EntityStorage::new(result);
            let cache = Cache::new().await?;
            let indexer = KgIndexer::new(entity_storage, cache);

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
