//! Hermes Spaces Transformer
//!
//! Consumes space-related events from hermes-substream via hermes-relay and
//! transforms them into Hermes protobuf messages for publication to Kafka.
//!
//! ## Event Types Handled
//!
//! - `SPACE_REGISTERED` - new space registrations -> `space.creations` topic
//! - `SUBSPACE_ADDED` - trust extensions -> `space.trust.extensions` topic
//! - `SUBSPACE_REMOVED` - trust revocations -> `space.trust.extensions` topic
//!
//! ## Configuration
//!
//! Environment variables:
//! - `SUBSTREAMS_ENDPOINT` - Substreams endpoint URL (required)
//! - `SUBSTREAMS_API_TOKEN` - Auth token for substreams (optional)
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_USERNAME` - SASL username for managed Kafka (optional)
//! - `KAFKA_PASSWORD` - SASL password for managed Kafka (optional)
//! - `KAFKA_SSL_CA_PEM` - Custom CA cert for SSL (optional)
//! - `START_BLOCK` - Block number to start from (default: 0)
//! - `END_BLOCK` - Block number to stop at (default: 0, meaning live streaming)

mod conversion;
mod kafka;
mod transformer;

use std::env;

use anyhow::Result;

use hermes_relay::{HermesModule, Sink};

use kafka::create_producer;
use transformer::SpacesTransformer;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Hermes Spaces Transformer starting...");

    // Parse configuration from environment
    let endpoint = env::var("SUBSTREAMS_ENDPOINT")
        .expect("SUBSTREAMS_ENDPOINT environment variable is required");
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let start_block: i64 = env::var("START_BLOCK")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .expect("START_BLOCK must be a valid integer");
    let end_block: u64 = env::var("END_BLOCK")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .expect("END_BLOCK must be a valid integer");

    println!("Configuration:");
    println!("  Substreams endpoint: {}", endpoint);
    println!("  Kafka broker: {}", broker);
    println!("  Start block: {}", start_block);
    println!(
        "  End block: {}",
        if end_block == 0 {
            "live streaming".to_string()
        } else {
            end_block.to_string()
        }
    );

    // Create Kafka producer
    println!("\nConnecting to Kafka broker...");
    let producer = create_producer(&broker, "hermes-spaces")?;
    println!("Connected to Kafka broker");

    // Create and run the transformer
    let transformer = SpacesTransformer::new(producer);

    println!("\nStarting spaces transformer...");
    println!("Subscribing to module: {}", HermesModule::Actions);
    println!("Filtering for: SPACE_REGISTERED, SUBSPACE_ADDED, SUBSPACE_REMOVED");
    println!();

    // Run the transformer - this will stream events and process them
    transformer
        .run(&endpoint, HermesModule::Actions, start_block, end_block)
        .await?;

    println!("\nSpaces transformer finished.");

    Ok(())
}
