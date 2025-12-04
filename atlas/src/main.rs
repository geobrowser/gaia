//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events and computes transitive and canonical graphs.

use atlas::mock_substream::MockSubstream;
use atlas::events::{SpaceTopologyEvent, SpaceTopologyPayload};

fn main() {
    println!("Starting Atlas topology processor...");

    // Create mock substream and generate initial topology
    let mut substream = MockSubstream::new();
    let events = substream.generate_topology(10);

    println!("Generated {} events from mock substream", events.len());
    println!("Root space: {:?}", hex::encode(substream.root_space_id()));
    println!("Root topic: {:?}", hex::encode(substream.root_topic_id()));
    println!();

    // Process each event
    for (i, event) in events.iter().enumerate() {
        process_event(i, event);
    }

    println!();
    println!("Processed {} events", events.len());
    println!("Total spaces: {}", substream.spaces().len());
    println!("Total topics: {}", substream.topics().len());
}

/// Process a single topology event
fn process_event(index: usize, event: &SpaceTopologyEvent) {
    let block = event.meta.block_number;

    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            println!(
                "[{}] Block {}: SpaceCreated {{ space: {}, topic: {} }}",
                index,
                block,
                hex::encode(created.space_id),
                hex::encode(created.topic_id),
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let extension_type = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    format!("Verified -> {}", hex::encode(target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    format!("Related -> {}", hex::encode(target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    format!("Subtopic -> {}", hex::encode(target_topic_id))
                }
            };
            println!(
                "[{}] Block {}: TrustExtended {{ source: {}, {} }}",
                index,
                block,
                hex::encode(extended.source_space_id),
                extension_type,
            );
        }
    }
}
