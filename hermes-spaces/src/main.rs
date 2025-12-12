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
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_USERNAME` - SASL username for managed Kafka (optional)
//! - `KAFKA_PASSWORD` - SASL password for managed Kafka (optional)
//! - `KAFKA_SSL_CA_PEM` - Custom CA cert for SSL (optional)

mod conversion;
mod kafka;
mod transformer;

use std::env;

use anyhow::Result;

use hermes_relay::{HermesModule, Sink, StreamSource};

use kafka::create_producer;
use transformer::SpacesTransformer;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Hermes Spaces Transformer starting...");

    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());

    println!("Configuration:");
    println!("  Kafka broker: {}", broker);

    // Create Kafka producer
    println!("\nConnecting to Kafka broker...");
    let producer = create_producer(&broker, "hermes-spaces")?;
    println!("Connected to Kafka broker");

    // Create the transformer
    let transformer = SpacesTransformer::new(producer);

    println!("\nStarting spaces transformer with mock data...");
    println!("Subscribing to module: {}", HermesModule::Actions);
    println!("Filtering for: SPACE_REGISTERED, SUBSPACE_ADDED, SUBSPACE_REMOVED");
    println!();

    // Run the transformer with mock data
    transformer.run(StreamSource::mock()).await?;

    println!("\nSpaces transformer finished.");

    Ok(())
}
