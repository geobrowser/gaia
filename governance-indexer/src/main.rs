use std::{env, io::Error};

use chrono::{DateTime, Utc};
use dotenv::dotenv;
use stream::Sink;

const PKG_FILE: &str = "geo_substream.spkg";
const MODULE_NAME: &str = "geo_out";
const START_BLOCK: i64 = 53821;

type GovernanceIndexerError = Error;

struct BlockMetadata {
    pub cursor: String,
    pub block_number: u64,
    pub timestamp: DateTime<Utc>,
    pub request_id: String,
}

struct GovernanceData {
    pub block: BlockMetadata,
}

struct GovernanceIndexer {}

impl GovernanceIndexer {
    pub fn new() -> Self {
        KgIndexer {}
    }
}

impl Sink<GovernanceData> for GovernanceIndexer {
    type Error = GovernanceIndexerError;

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

        // You can decode the actual Any type received using this code:
        //
        //     let value = GeneratedStructName::decode(output.value.as_slice())?;
        //
        // Where GeneratedStructName is the Rust code generated for the Protobuf representing
        // your type, so you will need generate it using `substreams protogen` and import it from the
        // `src/pb` folder.

        let clock = block_data.clock.as_ref().unwrap();
        let timestamp = clock.timestamp.as_ref().unwrap();
        let date = DateTime::from_timestamp(timestamp.seconds, timestamp.nanos as u32)
            .expect("received timestamp should always be valid");

        println!(
            "Block #{} - Payload {} ({} bytes) - Drift {}s",
            clock.number,
            output.type_url.replace("type.googleapis.com/", ""),
            output.value.len(),
            date.signed_duration_since(chrono::offset::Utc::now())
                .num_seconds()
                * -1
        );

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    dotenv().ok();

    let indexer = KgIndexer::new();

    let endpoint_url = env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");

    let _result = indexer
        .run(&endpoint_url, PKG_FILE, MODULE_NAME, START_BLOCK, 0)
        .await;

    Ok(())
}
