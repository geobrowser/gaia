//! Example demonstrating OTLP telemetry export.
//!
//! This example exports traces via OTLP gRPC to the configured endpoint.
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

use hermes_instrumentation::{Backend, Config, Instrument, info, info_span};

fn process_item(item_id: u32) {
    info!(item_id, "Processing item");
}

async fn fetch_data(source: &str) {
    info!(source, "Fetching data");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    info!("Data fetched successfully");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize telemetry with OTLP backend
    // Default Jaeger OTLP endpoint (no auth headers needed)
    hermes_instrumentation::init(Config {
        namespace: "example-service",
        backend: Backend::Otlp {
            endpoint: "http://localhost:4317",
            headers: &[],
        },
    })?;

    info!("Application starting");

    // Demonstrate explicit span instrumentation for sync code
    for i in 1..=3 {
        info_span!("process_item", item_id = i).in_scope(|| {
            process_item(i);
        });
    }

    // Demonstrate explicit span instrumentation for async code
    async {
        fetch_data("database").await;
    }
    .instrument(info_span!("fetch_data", source = "database"))
    .await;

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
