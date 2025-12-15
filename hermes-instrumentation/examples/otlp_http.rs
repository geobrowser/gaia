//! Example demonstrating OTLP telemetry export over HTTP.
//!
//! Use this for cloud providers like Axiom that require HTTP.
//!
//! To test with Axiom:
//!
//! 1. Create a dataset at https://app.axiom.co
//! 2. Create an API token with ingest permissions
//! 3. Set environment variables:
//!    ```sh
//!    export AXIOM_TOKEN="xaat-xxx"
//!    export AXIOM_DATASET="my-traces"
//!    ```
//! 4. Run this example:
//!    ```sh
//!    cargo run -p hermes-instrumentation --example otlp_http
//!    ```
//!
//! Run with: cargo run -p hermes-instrumentation --example otlp_http

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
    // Initialize telemetry with OTLP HTTP backend for Axiom
    // Set debug: true to also emit OTEL spans to stdout
    //
    // For production, use environment variables:
    //   AXIOM_TOKEN and AXIOM_DATASET
    hermes_instrumentation::init(Config::new(
        "example-service",
        Backend::OtlpHttp {
            endpoint: "https://api.axiom.co/v1/traces".into(),
            headers: vec![
                // Replace with your actual token, or use env vars in production
                ("Authorization".into(), "Bearer xaat-your-token-here".into()),
                ("X-Axiom-Dataset".into(), "your-dataset-name".into()),
            ],
            debug: true,
        },
    ))?;

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
