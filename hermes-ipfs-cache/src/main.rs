//! Hermes IPFS Cache binary
//!
//! Pre-fetches IPFS content for EditsPublished events from hermes-substream.

use std::env;

use dotenv::dotenv;
use hermes_ipfs_cache::{
    cache::{Cache, Storage},
    IpfsCacheSink,
};
use hermes_relay::Sink;
use ipfs::IpfsClient;

/// Default start block for the IPFS cache.
/// This should be set to the block where the Space Registry was deployed.
const DEFAULT_START_BLOCK: i64 = 0;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load configuration from environment
    let ipfs_gateway = env::var("IPFS_GATEWAY").expect("IPFS_GATEWAY not set");
    let endpoint_url = env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT not set");
    let start_block = env::var("START_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_START_BLOCK);
    let end_block = env::var("END_BLOCK")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0); // 0 means stream forever

    tracing::info!(
        endpoint = %endpoint_url,
        ipfs_gateway = %ipfs_gateway,
        start_block = start_block,
        end_block = end_block,
        "Starting Hermes IPFS Cache"
    );

    // Initialize storage and cache
    let storage = Storage::new().await?;
    let cache = Cache::new(storage);

    // Initialize IPFS client
    let ipfs = IpfsClient::new(&ipfs_gateway);

    // Create and run the sink
    let sink = IpfsCacheSink::new(cache, ipfs);

    sink.run(
        &endpoint_url,
        IpfsCacheSink::module(),
        start_block,
        end_block,
    )
    .await?;

    Ok(())
}
