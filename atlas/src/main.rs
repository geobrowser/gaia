//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events, computes canonical graphs,
//! and publishes updates to Kafka.

use std::env;

use atlas::convert::convert_mock_blocks;
use atlas::events::{SpaceId, SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};

// Use the shared mock_substream crate
use mock_substream::test_topology;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let topic = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "topology.canonical".to_string());

    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                     Atlas Topology Processor                                 ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Kafka broker: {}", broker);
    println!("Output topic: {}", topic);
    println!();

    // Set up Kafka producer
    let producer = AtlasProducer::new(&broker, &topic)?;
    let emitter = CanonicalGraphEmitter::new(producer);

    // Generate deterministic topology from shared mock_substream crate
    let blocks = test_topology::generate();
    let events = convert_mock_blocks(&blocks);

    let root_space = test_topology::ROOT_SPACE_ID;

    println!(
        "Generated {} topology events from mock substream",
        events.len()
    );
    println!("Root space: {}", format_space_id(root_space));
    println!();

    // Create graph state and processors
    let mut state = GraphState::new();
    let mut transitive = TransitiveProcessor::new();
    let mut canonical_processor = CanonicalProcessor::new(root_space);

    // Process each event
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Processing Events                                                            │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");

    let mut emit_count = 0;

    for (i, event) in events.iter().enumerate() {
        print_event(i, event);

        // Update transitive cache based on event
        transitive.handle_event(event, &state);

        // Apply event to graph state
        state.apply_event(event);

        // Compute canonical graph and emit if changed
        if let Some(graph) = canonical_processor.compute(&state, &mut transitive) {
            emitter.emit(&graph, &event.meta)?;
            emit_count += 1;
            println!(
                "│      └─▶ Emitted canonical graph update ({} nodes)",
                graph.len()
            );
        }
    }
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    println!();
    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Summary                                                                      │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");
    println!(
        "│ Total spaces:        {:>4}                                                    │",
        state.space_count()
    );
    println!(
        "│ Explicit edges:      {:>4}                                                    │",
        state.explicit_edge_count()
    );
    println!(
        "│ Topic edges:         {:>4}                                                    │",
        state.topic_edge_count()
    );
    println!(
        "│ Kafka messages sent: {:>4}                                                    │",
        emit_count
    );
    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    println!();
    println!("Atlas processing complete.");

    Ok(())
}

/// Format a space ID with a friendly name if known
fn format_space_id(id: SpaceId) -> String {
    let last_byte = id[15];
    let name = match last_byte {
        0x01 => "Root",
        0x0A => "A",
        0x0B => "B",
        0x0C => "C",
        0x0D => "D",
        0x0E => "E",
        0x0F => "F",
        0x10 => "G",
        0x11 => "H",
        0x12 => "I",
        0x13 => "J",
        0x20 => "X",
        0x21 => "Y",
        0x22 => "Z",
        0x23 => "W",
        0x30 => "P",
        0x31 => "Q",
        0x40 => "S",
        _ => return format!("{:.8}…", hex::encode(id)),
    };
    format!("{} (0x{:02x})", name, last_byte)
}

/// Format a topic ID with a friendly name if known
fn format_topic_id(id: &[u8; 16]) -> String {
    let last_byte = id[15];
    let name = match last_byte {
        0x02 => "T_Root",
        0x8A => "T_A",
        0x8B => "T_B",
        0x8C => "T_C",
        0x8D => "T_D",
        0x8E => "T_E",
        0x8F => "T_F",
        0x90 => "T_G",
        0x91 => "T_H",
        0x92 => "T_I",
        0x93 => "T_J",
        0xA0 => "T_X",
        0xA1 => "T_Y",
        0xA2 => "T_Z",
        0xA3 => "T_W",
        0xB0 => "T_P",
        0xB1 => "T_Q",
        0xC0 => "T_S",
        0xF0 => "T_SHARED",
        _ => return format!("{:.8}…", hex::encode(id)),
    };
    format!("{} (0x{:02x})", name, last_byte)
}

/// Print a single topology event
fn print_event(index: usize, event: &SpaceTopologyEvent) {
    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            println!(
                "│ [{:2}] SpaceCreated: {} announces {}",
                index,
                format_space_id(created.space_id),
                format_topic_id(&created.topic_id),
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let extension_str = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    format!("──verified──▶ {}", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    format!("──related──▶ {}", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    format!("──topic──▶ {}", format_topic_id(target_topic_id))
                }
            };
            println!(
                "│ [{:2}] TrustExtended: {} {}",
                index,
                format_space_id(extended.source_space_id),
                extension_str,
            );
        }
    }
}
