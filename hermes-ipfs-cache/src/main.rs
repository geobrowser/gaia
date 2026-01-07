//! Hermes IPFS Cache binary
//!
//! Pre-fetches IPFS content for EditsPublished events from hermes-substream.
//!
//! ## Configuration
//!
//! Environment variables:
//! - `USE_MOCK` - Set to "true" or "1" to use mock data (default: false)
//!
//! Required in live mode:
//! - `DATABASE_URL` - PostgreSQL connection URL for cache storage
//! - `IPFS_GATEWAY_URL` - IPFS gateway URL (e.g., https://ipfs.io/ipfs/)
//! - `SUBSTREAMS_ENDPOINT` - Substreams endpoint URL (e.g., geotest.substreams.pinax.network:443)
//! - `SUBSTREAMS_API_TOKEN` - API token for substreams authentication
//!
//! Optional:
//! - `SUBSTREAMS_START_BLOCK` - First block to consume (default: 88109)
//! - `SUBSTREAMS_END_BLOCK` - Last block to consume (default: u64::MAX for continuous)

use std::collections::HashMap;
use std::env;

use hermes_instrumentation::{Backend, Config, info};
use hermes_ipfs_cache::{IpfsCacheSink, cache::CacheSource};
use hermes_relay::{HermesModule, Sink, StreamSource};
use ipfs::IpfsSource;
use wire::pb::grc20::Edit;

/// Generate mock edits matching the test topology IPFS hashes.
///
/// These correspond to the `edit_published` events in
/// `hermes_relay::source::mock_events::test_topology::generate()`.
fn test_topology_edits() -> HashMap<String, Edit> {
    let mut edits = HashMap::new();

    // Helper to create a simple edit
    let make_edit = |name: &str| Edit {
        id: name.as_bytes().to_vec(),
        name: name.to_string(),
        ops: vec![],
        authors: vec![],
        language: None,
    };

    // These CIDs match the ones in mock_events::test_topology::generate()
    edits.insert(
        "QmRootEdit1CreatePersons".to_string(),
        make_edit("Root Edit 1: Create Persons"),
    );
    edits.insert(
        "QmRootEdit2AddDescriptions".to_string(),
        make_edit("Root Edit 2: Add Descriptions"),
    );
    edits.insert(
        "QmSpaceAEdit1CreateOrg".to_string(),
        make_edit("Space A Edit 1: Create Org"),
    );
    edits.insert(
        "QmSpaceAEdit2CreateRelations".to_string(),
        make_edit("Space A Edit 2: Create Relations"),
    );
    edits.insert(
        "QmSpaceBEdit1CreateDoc".to_string(),
        make_edit("Space B Edit 1: Create Doc"),
    );
    edits.insert(
        "QmSpaceCEdit1CreateTopic".to_string(),
        make_edit("Space C Edit 1: Create Topic"),
    );

    edits
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
fn build_telemetry_config() -> Config {
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

    Config::new("hermes-ipfs-cache", backend)
}

fn main() -> anyhow::Result<()> {
    // Load .env file if present (ignored in production)
    dotenv::dotenv().ok();

    // Initialize telemetry BEFORE tokio runtime starts.
    let _telemetry = hermes_instrumentation::init(build_telemetry_config())?;

    // Create and run the tokio runtime manually
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    info!("Hermes IPFS Cache starting");

    // Determine mode: mock (default for backwards compat during dev) or live
    let use_mock = env::var("USE_MOCK")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);

    if use_mock {
        info!("Using mock data sources");

        // Create mock cache (in-memory)
        let cache = CacheSource::mock().into_cache().await?;

        // Create mock IPFS source with test topology edits
        let ipfs_source = IpfsSource::mock(test_topology_edits());

        // Create and run the sink with mock data
        let sink = IpfsCacheSink::new(cache, ipfs_source);
        sink.run(StreamSource::mock()).await?;
    } else {
        // Live mode - connect to real substreams, IPFS, and database
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL must be set for live mode");

        let ipfs_gateway =
            env::var("IPFS_GATEWAY_URL").expect("IPFS_GATEWAY_URL must be set for live mode");

        let endpoint =
            env::var("SUBSTREAMS_ENDPOINT").expect("SUBSTREAMS_ENDPOINT must be set for live mode");

        let start_block: i64 = env::var("SUBSTREAMS_START_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(81809);

        let end_block: u64 = env::var("SUBSTREAMS_END_BLOCK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(u64::MAX);

        info!(
            endpoint = %endpoint,
            start_block = start_block,
            end_block = end_block,
            ipfs_gateway = %ipfs_gateway,
            "Using live substreams source"
        );

        // Create PostgreSQL-backed cache
        let cache = CacheSource::live(&database_url).into_cache().await?;

        // Create live IPFS client
        let ipfs_source = IpfsSource::live(&ipfs_gateway);

        // Create stream source for EditsPublished module
        let source = StreamSource::live(
            endpoint,
            HermesModule::EditsPublished,
            start_block,
            end_block,
        );

        // Create and run the sink
        let sink = IpfsCacheSink::new(cache, ipfs_source);
        sink.run(source).await?;
    }

    info!("Hermes IPFS Cache finished");

    Ok(())
}
