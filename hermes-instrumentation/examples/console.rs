//! Example demonstrating console telemetry output.
//!
//! Run with: cargo run -p hermes-instrumentation --example console

use hermes_instrumentation::{Backend, Config, Instrument, info, info_span};

fn process_item(item_id: u32) {
    info!(item_id, "Processing item");
}

async fn fetch_data(source: &str) {
    info!(source, "Fetching data");
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    info!("Data fetched successfully");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize telemetry with console backend
    hermes_instrumentation::init(Config {
        namespace: "example-service",
        backend: Backend::Console,
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
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        info!("Batch complete");
    }
    .instrument(span)
    .await;

    info!("Application finished");

    Ok(())
}
