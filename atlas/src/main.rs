//! Atlas - Space Topology Processor
//!
//! Entry point for the Atlas graph processing pipeline.
//! Consumes space topology events from hermes-relay, computes canonical graphs,
//! and publishes updates to Kafka.
//!
//! ## Configuration
//!
//! Environment variables:
//! - `KAFKA_BROKER` - Kafka broker address (default: localhost:9092)
//! - `KAFKA_TOPIC` - Output topic for canonical graph updates (default: topology.canonical)
//! - `OTEL_URL` - OTLP HTTP endpoint (e.g., "https://api.axiom.co/v1/traces")
//! - `OTEL_TOKEN` - Bearer token for authentication
//! - `OTEL_DATASET` - Dataset name (sent as X-Axiom-Dataset header)
//! - `OTEL_DEBUG` - Set to "true" to also emit spans to stdout

use std::env;
use std::sync::Mutex;

use atlas::convert::convert_action;
use atlas::events::{BlockMetadata, SpaceId, SpaceTopologyEvent, SpaceTopologyPayload};
use atlas::graph::{CanonicalProcessor, GraphState, TransitiveProcessor};
use atlas::kafka::{AtlasProducer, CanonicalGraphEmitter};
use hermes_instrumentation::{debug, info, info_span, Instrument};
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

        info!(
            spaces = state.space_count(),
            explicit_edges = state.explicit_edge_count(),
            topic_edges = state.topic_edge_count(),
            kafka_messages = emit_count,
            "Processing complete"
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

        // Process each action within a block span
        let action_count = actions.actions.len();
        async {
            for action in &actions.actions {
                if let Some(event) = convert_action(action, &meta) {
                    self.process_event(&event)?;
                }
            }
            Ok::<(), AtlasError>(())
        }
        .instrument(info_span!(
            "process_block",
            block_number,
            cursor = %meta.cursor,
            action_count
        ))
        .await
    }
}

impl AtlasSink {
    fn process_event(&self, event: &SpaceTopologyEvent) -> Result<(), AtlasError> {
        let event_type = match &event.payload {
            SpaceTopologyPayload::SpaceCreated(_) => "SpaceCreated",
            SpaceTopologyPayload::TrustExtended(_) => "TrustExtended",
        };

        let _span = info_span!("process_event", event_type).entered();

        let mut state = self.state.lock().unwrap();
        let mut transitive = self.transitive.lock().unwrap();
        let mut canonical_processor = self.canonical_processor.lock().unwrap();
        let mut event_count = self.event_count.lock().unwrap();
        let mut emit_count = self.emit_count.lock().unwrap();

        // Log the event
        log_event(*event_count, event);
        *event_count += 1;

        // Update transitive cache based on event
        transitive.handle_event(event, &state);

        // Apply event to graph state
        state.apply_event(event);

        // Compute canonical graph and emit if changed
        if let Some(graph) = canonical_processor.compute(&state, &mut transitive) {
            let node_count = graph.len();
            self.emitter
                .emit(&graph, &event.meta)
                .map_err(|e| AtlasError::KafkaError(e.to_string()))?;
            *emit_count += 1;
            info!(node_count, "Emitted canonical graph update");
        }

        Ok(())
    }
}

/// Build telemetry configuration from environment variables.
///
/// Environment variables:
/// - `OTEL_URL` - OTLP HTTP endpoint (e.g., "https://api.axiom.co/v1/traces")
/// - `OTEL_TOKEN` - Bearer token for authentication
/// - `OTEL_DATASET` - Dataset name (sent as X-Axiom-Dataset header)
/// - `OTEL_DEBUG` - Set to "true" to also emit spans to stdout
///
/// If `OTEL_URL` is not set, falls back to Console backend.
fn build_telemetry_config() -> hermes_instrumentation::Config {
    use hermes_instrumentation::{Backend, Config};

    let backend = match env::var("OTEL_URL") {
        Ok(endpoint) => {
            let mut headers = Vec::new();

            if let Ok(token) = env::var("OTEL_TOKEN") {
                headers.push(("Authorization".into(), format!("Bearer {}", token)));
            }

            let dataset = env::var("OTEL_DATASET").ok();
            if let Some(ref dataset) = dataset {
                headers.push(("X-Axiom-Dataset".into(), dataset.clone()));
            }

            let debug = env::var("OTEL_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            let has_auth = headers.iter().any(|(k, _)| k == "Authorization");
            println!(
                "Telemetry: OTLP HTTP -> {} (dataset: {}, auth: {}, debug: {})",
                endpoint,
                dataset.as_deref().unwrap_or("none"),
                if has_auth { "yes" } else { "no" },
                if debug { "yes" } else { "no" }
            );

            Backend::OtlpHttp {
                endpoint,
                headers,
                debug,
            }
        }
        _ => {
            println!("Telemetry: Console (set OTEL_URL to enable OTLP export)");
            Backend::Console
        }
    };

    Config::new("atlas", backend)
}

fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignored in production)
    dotenv::dotenv().ok();

    // Initialize telemetry BEFORE tokio runtime starts.
    // The OTLP HTTP backend uses a blocking HTTP client that creates its own
    // internal tokio runtime. Tokio runtimes cannot be nested, so we must
    // initialize telemetry before creating our application's runtime.
    //
    // Keep the guard alive until the end of main to ensure spans are flushed.
    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    // Create and run the tokio runtime manually (instead of #[tokio::main])
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let broker = env::var("KAFKA_BROKER").unwrap_or_else(|_| "localhost:9092".to_string());
    let topic = env::var("KAFKA_TOPIC").unwrap_or_else(|_| "topology.canonical".to_string());

    info!("Atlas Topology Processor starting");
    info!(kafka_broker = %broker, kafka_topic = %topic, "Configuration loaded");

    // Set up Kafka producer
    debug!("Connecting to Kafka broker");
    let producer = AtlasProducer::new(&broker, &topic)?;
    let emitter = CanonicalGraphEmitter::new(producer);
    info!("Connected to Kafka broker");

    // Create the sink with root space from test topology
    let sink = AtlasSink::new(ROOT_SPACE_ID, emitter);

    info!("Starting event processing");

    // Run with mock data source (all events in a single block)
    // In production, this would be StreamSource::live(endpoint_url, module, start_block, end_block)
    sink.run(StreamSource::mock()).await?;

    sink.summary();

    info!("Atlas processing complete");

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
        _ => return format!("{:.8}...", hex::encode(id)),
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
        _ => return format!("{:.8}...", hex::encode(id)),
    };
    format!("{} (0x{:02x})", name, last_byte)
}

/// Log a topology event with structured fields
fn log_event(index: usize, event: &SpaceTopologyEvent) {
    match &event.payload {
        SpaceTopologyPayload::SpaceCreated(created) => {
            debug!(
                index,
                space_id = %format_space_id(created.space_id),
                topic_id = %format_topic_id(&created.topic_id),
                "SpaceCreated"
            );
        }
        SpaceTopologyPayload::TrustExtended(extended) => {
            let (extension_type, target) = match &extended.extension {
                atlas::events::TrustExtension::Verified { target_space_id } => {
                    ("verified", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Related { target_space_id } => {
                    ("related", format_space_id(*target_space_id))
                }
                atlas::events::TrustExtension::Subtopic { target_topic_id } => {
                    ("topic", format_topic_id(target_topic_id))
                }
            };
            debug!(
                index,
                source_space_id = %format_space_id(extended.source_space_id),
                extension_type,
                target = %target,
                "TrustExtended"
            );
        }
    }
}
