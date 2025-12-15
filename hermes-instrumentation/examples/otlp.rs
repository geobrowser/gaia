//! Example demonstrating OTLP telemetry export.
//!
//! This example exports traces via OTLP gRPC to the configured endpoint.
//! It also outputs to console so you can see the spans being created.
//!
//! To test with a local collector:
//!
//! ```sh
//! # Option 1: Jaeger all-in-one with OTLP support
//! docker run -d --name jaeger \
//!   -p 4317:4317 \
//!   -p 4318:4318 \
//!   -p 16686:16686 \
//!   jaegertracing/all-in-one:latest
//!
//! # Then run this example
//! cargo run -p hermes-instrumentation --example otlp
//!
//! # View traces at http://localhost:16686
//! ```
//!
//! Run with: cargo run -p hermes-instrumentation --example otlp

use hermes_instrumentation::{info, info_span, instrument, Backend, Config, Instrument};

#[instrument]
fn process_item(item_id: u32) {
    info!(item_id, "Processing item");
}

#[instrument]
async fn fetch_data(source: &str) {
    info!(source, "Fetching data");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    info!("Data fetched successfully");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize telemetry with OTLP backend
    // Default Jaeger OTLP endpoint
    hermes_instrumentation::init(Config {
        namespace: "example-service",
        backend: Backend::Otlp {
            endpoint: "http://localhost:4317",
        },
    })?;

    info!("Application starting");

    // Demonstrate sync instrumentation
    for i in 1..=3 {
        process_item(i);
    }

    // Demonstrate async instrumentation
    fetch_data("database").await;

    // Demonstrate manual span creation
    let span = info_span!("batch_operation", batch_size = 10);
    async {
        info!("Starting batch");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        info!("Batch complete");
    }
    .instrument(span)
    .await;

    info!("Application finished");

    // Give time for spans to be exported
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Shutdown telemetry to flush remaining spans
    hermes_instrumentation::shutdown();

    Ok(())
}
