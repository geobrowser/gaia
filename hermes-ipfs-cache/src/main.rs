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
//! - `SUBSTREAMS_START_BLOCK` - First block to consume (default: 82655)
//! - `SUBSTREAMS_END_BLOCK` - Last block to consume (default: u64::MAX for continuous)

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;

use grc_20::{Edit as Grc20Edit, encode_edit};
use hermes_instrumentation::{Backend, Config, info};
use hermes_ipfs_cache::{IpfsCacheSink, cache::CacheSource};
use hermes_relay::{HermesModule, Sink, StreamSource};
use ipfs::IpfsSource;

/// Generate mock edits matching the test topology IPFS hashes.
///
/// These correspond to the `edit_published` events in
/// `hermes_relay::source::mock_events::test_topology::generate()`.
///
/// Returns CID → GRC-20 v2 encoded bytes.
fn test_topology_edits() -> HashMap<String, Vec<u8>> {
    let mut edits = HashMap::new();

    // Helper to create a simple GRC-20 v2 edit
    let make_edit = |id: [u8; 16], name: &str| -> Vec<u8> {
        let edit = Grc20Edit {
            id,
            name: Cow::Owned(name.to_string()),
            authors: vec![[0x01; 16]], // Mock author
            created_at: 1700000000,
            ops: vec![],
        };
        encode_edit(&edit).expect("Failed to encode test edit")
    };

    // These CIDs match the ones in mock_events::test_topology::generate()
    edits.insert(
        "QmRootEdit1CreatePersons".to_string(),
        make_edit([0xE1; 16], "Root Edit 1: Create Persons"),
    );
    edits.insert(
        "QmRootEdit2AddDescriptions".to_string(),
        make_edit([0xE2; 16], "Root Edit 2: Add Descriptions"),
    );
    edits.insert(
        "QmSpaceAEdit1CreateOrg".to_string(),
        make_edit([0xEA; 16], "Space A Edit 1: Create Org"),
    );
    edits.insert(
        "QmSpaceAEdit2CreateRelations".to_string(),
        make_edit([0xEB; 16], "Space A Edit 2: Create Relations"),
    );
    edits.insert(
        "QmSpaceBEdit1CreateDoc".to_string(),
        make_edit([0xEC; 16], "Space B Edit 1: Create Doc"),
    );
    edits.insert(
        "QmSpaceCEdit1CreateTopic".to_string(),
        make_edit([0xED; 16], "Space C Edit 1: Create Topic"),
    );

    edits
}

/// Build telemetry configuration from environment variables.
///
/// Environment variables:
/// - `SENTRY_DSN` - Sentry DSN/ingest URL
/// - `SENTRY_TRACES_SAMPLE_RATE` - Sampling rate (0.0 - 1.0)
/// - `SENTRY_SEND_DEFAULT_PII` - Set to "true" to include PII
/// - `SENTRY_ENVIRONMENT` - Environment tag (e.g., "prod", "staging")
/// - `SENTRY_RELEASE` - Release name (e.g., "service@1.2.3")
/// - `SENTRY_DEBUG` - Set to "true" to also emit spans to stdout
///
/// If `SENTRY_DSN` is not set, falls back to Console backend.
fn build_telemetry_config() -> Config {
    let backend = match env::var("SENTRY_DSN") {
        Ok(dsn) => {
            let traces_sample_rate = env::var("SENTRY_TRACES_SAMPLE_RATE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0);
            let send_default_pii = env::var("SENTRY_SEND_DEFAULT_PII")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);
            let environment = env::var("SENTRY_ENVIRONMENT").ok();
            let release = env::var("SENTRY_RELEASE").ok();
            let debug = env::var("SENTRY_DEBUG")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false);

            println!(
                "Telemetry: Sentry (env: {}, release: {}, debug: {})",
                environment.as_deref().unwrap_or("none"),
                release.as_deref().unwrap_or("none"),
                if debug { "yes" } else { "no" }
            );

            Backend::Sentry {
                dsn,
                traces_sample_rate,
                send_default_pii,
                environment,
                release,
                debug,
                axiom: hermes_instrumentation::AxiomConfig::from_env(),
            }
        }
        _ => {
            println!("Telemetry: Console (set SENTRY_DSN to enable Sentry)");
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

        // Create mock IPFS source with GRC-20 v2 test topology edits
        let ipfs_source = IpfsSource::mock_bytes(test_topology_edits());

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
            .unwrap_or(82655);

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

        // Create stream source for IpfsUris module (combined edits + proposals)
        let source = StreamSource::live(endpoint, HermesModule::IpfsUris, start_block, end_block);

        // Create and run the sink
        let sink = IpfsCacheSink::new(cache, ipfs_source);
        sink.run(source).await?;
    }

    info!("Hermes IPFS Cache finished");

    Ok(())
}
