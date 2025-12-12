//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events from hermes-relay, computes canonical graphs,
//! and publishes updates to Kafka.

use std::env;
use std::sync::Mutex;

use atlas::convert::convert_action;
use atlas::events::{BlockMetadata, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};
use hermes_relay::source::mock_events::test_topology::ROOT_SPACE_ID;
use hermes_relay::{Actions, Sink, StreamSource};
use prost::Message;

/// Atlas topology processor that implements the hermes-relay Sink trait.
struct AtlasSink {
    /// Graph state tracking all spaces and edges
    state: Mutex<GraphState>,
    /// Transitive closure processor
    transitive: Mutex<TransitiveProcessor>,
    /// Canonical graph processor
    canonical_processor: Mutex<CanonicalProcessor>,
    /// Kafka emitter for canonical graph updates
    emitter: CanonicalGraphEmitter,
    /// Event counter for logging
    event_count: Mutex<usize>,
    /// Emit counter for summary
    emit_count: Mutex<usize>,
}

impl AtlasSink {
    fn new(root_space: SpaceId, emitter: CanonicalGraphEmitter) -> Self {
        Self {
            state: Mutex::new(GraphState::new()),
            transitive: Mutex::new(TransitiveProcessor::new()),
            canonical_processor: Mutex::new(CanonicalProcessor::new(root_space)),
            emitter,
            event_count: Mutex::new(0),
            emit_count: Mutex::new(0),
        }
    }

    fn summary(&self) {
        let state = self.state.lock().unwrap();
        let emit_count = *self.emit_count.lock().unwrap();

        println!();
        println!(
            "┌──────────────────────────────────────────────────────────────────────────────┐"
        );
        println!(
            "│ Summary                                                                      │"
        );
        println!(
            "├──────────────────────────────────────────────────────────────────────────────┤"
        );
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
        println!(
            "└──────────────────────────────────────────────────────────────────────────────┘"
        );
    }
}

#[derive(Debug, thiserror::Error)]
enum AtlasError {
    #[error("Failed to decode actions: {0}")]
    DecodeError(#[from] prost::DecodeError),
    #[error("Kafka error: {0}")]
    KafkaError(String),
}

impl Sink for AtlasSink {
    type Error = AtlasError;

    async fn process_block_scoped_data(
        &self,
        data: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    ) -> Result<(), Self::Error> {
        // Extract block metadata
        let clock = data.clock.as_ref();
        let block_number = clock.map(|c| c.number).unwrap_or(0);
        let block_timestamp = clock
            .and_then(|c| c.timestamp.as_ref())
            .map(|t| t.seconds as u64)
            .unwrap_or(0);

        let meta = BlockMetadata {
            block_number,
            block_timestamp,
            tx_hash: String::new(),
            cursor: data.cursor.clone(),
        };

        // Decode actions from the block output
        let output = data
            .output
            .as_ref()
            .and_then(|o| o.map_output.as_ref())
            .map(|a| a.value.as_slice())
            .unwrap_or(&[]);

        if output.is_empty() {
            return Ok(());
        }

        let actions = Actions::decode(output)?;

        // Convert actions to topology events and process them
        for action in &actions.actions {
            if let Some(event) = convert_action(action, &meta) {
                self.process_event(&event)?;
            }
        }

        Ok(())
    }
}

impl AtlasSink {
    fn process_event(&self, event: &SpaceTopologyEvent) -> Result<(), AtlasError> {
        let mut state = self.state.lock().unwrap();
        let mut transitive = self.transitive.lock().unwrap();
        let mut canonical_processor = self.canonical_processor.lock().unwrap();
        let mut event_count = self.event_count.lock().unwrap();
        let mut emit_count = self.emit_count.lock().unwrap();

        // Log the event
        print_event(*event_count, event);
        *event_count += 1;

        // Update transitive cache based on event
        transitive.handle_event(event, &state);

        // Apply event to graph state
        state.apply_event(event);

        // Compute canonical graph and emit if changed
        if let Some(graph) = canonical_processor.compute(&state, &mut transitive) {
            self.emitter
                .emit(&graph, &event.meta)
                .map_err(|e| AtlasError::KafkaError(e.to_string()))?;
            *emit_count += 1;
            println!(
                "│      └─▶ Emitted canonical graph update ({} nodes)",
                graph.len()
            );
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Create the sink with root space from test topology
    let sink = AtlasSink::new(ROOT_SPACE_ID, emitter);

    println!("┌──────────────────────────────────────────────────────────────────────────────┐");
    println!("│ Processing Events                                                            │");
    println!("├──────────────────────────────────────────────────────────────────────────────┤");

    // Run with mock data source (all events in a single block)
    // In production, this would be StreamSource::live(endpoint_url, module, start_block, end_block)
    sink.run(StreamSource::mock()).await?;

    println!("└──────────────────────────────────────────────────────────────────────────────┘");

    sink.summary();

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
