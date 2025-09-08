use indexer_utils::get_blocklist;
use indexer_utils::id::derive_space_id;
use indexer_utils::network_ids::GEO;
use std::sync::Arc;
use std::{env, io::Error};
use stream::utils::BlockMetadata;
use thiserror::Error;
use tokio::task;
use wire::pb::chain::{EditPublished, GeoOutput};

use dotenv::dotenv;
use prost::Message;
use stream::Sink;
use tokio::sync::{Mutex, Semaphore};

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 56013;

mod cache;
use cache::{Cache, CacheItem};
use ipfs::IpfsClient;

type CacheIndexerError = Error;

pub struct EventData {
    pub block: BlockMetadata,
    pub edits_published: Vec<(EditPublished, Vec<EditPublished>)>,
}

struct CacheIndexer {
    semaphore: Arc<Semaphore>,
    cache: Arc<Mutex<Cache>>,
    ipfs: Arc<IpfsClient>,
}

impl CacheIndexer {
    pub fn new(cache: Cache, ipfs: IpfsClient) -> Self {
        CacheIndexer {
            cache: Arc::new(Mutex::new(cache)),
            ipfs: Arc::new(ipfs),
            semaphore: Arc::new(Semaphore::new(20)),
        }
    }
}

#[derive(Error, Debug)]
enum IndexerError {
    #[error("Cache indexer error: {0}")]
    Error(#[from] cache::CacheError),
}

impl Sink<EventData> for CacheIndexer {
    type Error = CacheIndexerError;

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        self.cache
            .lock()
            .await
            .load_cursor("ipfs_indexer")
            .await
            .map_err(|e| Error::new(std::io::ErrorKind::Other, e))
    }

    async fn persist_cursor(&self, cursor: String, block: u64) -> Result<(), Self::Error> {
        self.cache
            .lock()
            .await
            .persist_cursor("ipfs_indexer", &cursor, &block)
            .await
            .map_err(|e| Error::new(std::io::ErrorKind::Other, e))
    }

    async fn process_block_scoped_data(
        &self,
        block_data: &stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    ) -> Result<(), Self::Error> {
        let output = stream::utils::output(block_data);

        // @TODO: Parsing and decoding of event data should happen in a separate module.
        // This makes it so we can generate test data using these decoders and pass them
        // to any arbitrary handler. This gives us testing and prototyping by mocking the
        // events coming via the stream.

        // We should take the code to get the output and decode it into
        // a "GeoOutput" into it's own module that any Sink trait impl
        // can consume to get the decoded data from the substream.

        // We want to enable extensible governance actions. This means we should probably
        // distinguish between KG messages and governance messages.
        let geo = GeoOutput::decode(output.value.as_slice())?;

        let block_metadata = stream::utils::block_metadata(block_data);

        let block_timestamp_seconds: i64 = block_metadata.timestamp.parse().unwrap_or(0);
        let block_datetime = chrono::DateTime::from_timestamp(block_timestamp_seconds, 0)
            .unwrap_or_else(|| chrono::Utc::now())
            .with_timezone(&chrono::Local);
        let drift_str = stream::utils::format_drift(&block_metadata);

        println!(
            "Processing Block #{} [{}] - Payload {} ({} bytes) - Drift {} – Edits Published {}",
            block_metadata.block_number,
            block_datetime.format("%Y-%m-%d %H:%M:%S"),
            output.type_url.replace("type.googleapis.com/", ""),
            output.value.len(),
            drift_str,
            geo.edits_published.len()
        );

        for edit in geo.edits_published {
            if get_blocklist()
                .dao_addresses
                .contains(&edit.dao_address.as_str())
            {
                continue;
            }

            let permit = self.semaphore.clone().acquire_owned().await.unwrap();
            let cache = self.cache.clone();
            let ipfs = self.ipfs.clone();

            println!(
                "Processing cache entry for uri {} in block {}",
                edit.content_uri, block_metadata.block_number
            );

            let block_metadata = stream::utils::block_metadata(block_data);

            task::spawn(async move {
                process_edit_event(edit, &cache, &ipfs, &block_metadata).await?;
                drop(permit);
                Ok::<(), IndexerError>(())
            });
        }

        Ok(())
    }
}

async fn process_edit_event(
    edit: EditPublished,
    cache: &Arc<Mutex<Cache>>,
    ipfs: &Arc<IpfsClient>,
    block: &BlockMetadata,
) -> Result<(), IndexerError> {
    {
        let mut cache_instance = cache.lock().await;

        if cache_instance.has(&edit.content_uri).await? {
            return Ok(());
        }
    }

    let data = ipfs.get(&edit.content_uri).await;

    match data {
        Ok(result) => {
            let item = CacheItem {
                uri: edit.content_uri.clone(),
                block: block.timestamp.clone(),
                json: Some(result),
                space: derive_space_id(GEO, &edit.dao_address),
                is_errored: false,
            };

            let mut cache_instance = cache.lock().await;
            let res = cache_instance.put(&item).await;

            match res {
                Ok(_) => {
                    println!(
                        "Successfully wrote cid to cache {} for block {}",
                        edit.content_uri.clone(),
                        block.block_number,
                    );
                }
                Err(err) => {
                    println!("Err {:?}", err)
                }
            }
        }
        Err(error) => {
            println!(
                "Error writing decoded edit event to cache for uri {} in block {} {}",
                edit.content_uri, block.block_number, error
            );

            // We may receive events where the format of the ipfs contents is
            // invalid. We still write a cache item with an is_errored status
            // so that cache consumers can always read the cache to get either
            // the decoded state, or be notified that the event exists, but the
            // contents are invalid.
            let item = CacheItem {
                uri: edit.content_uri,
                block: block.timestamp.clone(),
                json: None,
                space: derive_space_id(GEO, &edit.dao_address),
                is_errored: true,
            };

            let mut cache_instance = cache.lock().await;
            cache_instance.put(&item).await?;
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    dotenv().ok();

    let ipfs_gateway = env::var("IPFS_GATEWAY").expect("IPFS_GATEWAY not set");
    let ipfs = IpfsClient::new(&ipfs_gateway);
    let storage = cache::Storage::new().await;

    match storage {
        Ok(result) => {
            let kv = cache::Cache::new(result);
            let indexer = CacheIndexer::new(kv, ipfs);

            let endpoint_url =
                env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");

            let _result = indexer
                .run(&endpoint_url, PKG_FILE, MODULE_NAME, START_BLOCK, 0)
                .await;
        }
        Err(err) => {
            println!("Error initializing stream {}", err);
        }
    }

    Ok(())
}
