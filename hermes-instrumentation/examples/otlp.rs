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

fn validate_item(item_id: u32) {
    info!(item_id, "Validating item");
}

fn save_item(item_id: u32) {
    info!(item_id, "Saving item");
}

fn process_item(item_id: u32) {
    info!(item_id, "Processing item");

    // Child spans for sub-operations
    info_span!("validate").in_scope(|| {
        validate_item(item_id);
    });

    info_span!("save").in_scope(|| {
        save_item(item_id);
    });
}

async fn fetch_record(id: u32) {
    info!(id, "Fetching record");
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
}

async fn fetch_data(source: &str) {
    info!(source, "Fetching data");

    // Child spans for individual record fetches
    for id in 1..=2 {
        async {
            fetch_record(id).await;
        }
        .instrument(info_span!("fetch_record", id))
        .await;
    }

    info!("Data fetched successfully");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize telemetry with OTLP backend
    // Default Jaeger OTLP endpoint (no auth headers needed)
    // Set debug: true to also emit OTEL spans to stdout
    hermes_instrumentation::init(Config {
        namespace: "example-service",
        backend: Backend::OtlpGrpc {
            endpoint: "http://localhost:4317",
            headers: &[],
            debug: true,
        },
    })?;

    info!("Application starting");

    // Parent span with child spans (sync)
    for i in 1..=2 {
        info_span!("process_item", item_id = i).in_scope(|| {
            process_item(i);
        });
    }

    // Parent span with child spans (async)
    async {
        fetch_data("database").await;
    }
    .instrument(info_span!("fetch_data", source = "database"))
    .await;

    // Nested async spans
    async {
        info!("Starting batch");

        for i in 1..=2 {
            async {
                info!(item = i, "Processing batch item");
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            .instrument(info_span!("batch_item", item = i))
            .await;
        }

        info!("Batch complete");
    }
    .instrument(info_span!("batch_operation", batch_size = 2))
    .await;

    info!("Application finished");

    // Give time for spans to be exported
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Shutdown telemetry to flush remaining spans
    hermes_instrumentation::shutdown();

    Ok(())
}
