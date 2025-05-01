use std::sync::Arc;
use std::{env, io::Error};
use tokio::task;

use chrono::{DateTime, Utc};
use dotenv::dotenv;
use prost::Message;
use stream::Sink;
use tokio::sync::Mutex;

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 881;

use grc20::pb::chain::{EditPublished, GeoOutput};

mod cache;
use cache::Cache;

type CacheIndexerError = Error;

struct BlockMetadata {
    pub cursor: String,
    pub block_number: u64,
    pub timestamp: DateTime<Utc>,
    pub request_id: String,
}

pub struct EventData {
    pub block: BlockMetadata,
    pub edits_published: Vec<(EditPublished, Vec<EditPublished>)>,
}

struct CacheIndexer {
    cache: Arc<Mutex<Cache>>,
}

impl CacheIndexer {
    pub fn new(cache: Cache) -> Self {
        CacheIndexer {
            cache: Arc::new(Mutex::new(cache)),
        }
    }
}

impl Sink<EventData> for CacheIndexer {
    type Error = CacheIndexerError;

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
        let output = block_data
            .output
            .as_ref()
            .unwrap()
            .map_output
            .as_ref()
            .unwrap();

        // @TODO: Parsing and decoding of event data should happen in a separate module.
        // This makes it so we can generate test data using these decoders and pass them
        // to any arbitrary handler. This gives us testing and prototyping by mocking the
        // events coming via the stream.

        let geo = GeoOutput::decode(output.value.as_slice())?;

        let clock = block_data.clock.as_ref().unwrap();
        let timestamp = clock.timestamp.as_ref().unwrap();
        let date = DateTime::from_timestamp(timestamp.seconds, timestamp.nanos as u32)
            .expect("received timestamp should always be valid");

        println!(
            "Block #{} - Payload {} ({} bytes) - Drift {}s – Edits Published {}",
            clock.number,
            output.type_url.replace("type.googleapis.com/", ""),
            output.value.len(),
            date.signed_duration_since(chrono::offset::Utc::now())
                .num_seconds()
                * -1,
            geo.edits_published.len()
        );

        let cache = self.cache.clone();

        task::spawn(async move {
            process_edit_event(geo.edits_published, &cache).await;
        });

        Ok(())
    }
}

async fn process_edit_event(event: Vec<EditPublished>, cache: &Arc<Mutex<Cache>>) {
    for edit in event {
        let mut cache_instance = cache.lock().await;
        cache_instance.put(&edit.content_uri, &edit.dao_address);

        let test = cache_instance.get(&edit.content_uri);
        println!("{:?}", test);
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    dotenv().ok();

    let kv = cache::Cache::new(cache::Storage::new());
    let indexer = CacheIndexer::new(kv);

    let endpoint_url = env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");

    let _result = indexer
        .run(&endpoint_url, PKG_FILE, MODULE_NAME, START_BLOCK, 0)
        .await;

    Ok(())
}
