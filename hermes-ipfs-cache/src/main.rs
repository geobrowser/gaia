//! Hermes IPFS Cache binary
//!
//! Pre-fetches IPFS content for EditsPublished events from hermes-substream.

use std::collections::HashMap;

use hermes_ipfs_cache::{cache::CacheSource, IpfsCacheSink};
use hermes_relay::{Sink, StreamSource};
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    tracing::info!("Starting Hermes IPFS Cache with mock data");

    // Create mock cache (in-memory)
    let cache = CacheSource::mock().into_cache().await?;

    // Create mock IPFS source with test topology edits
    let ipfs_source = IpfsSource::mock(test_topology_edits());

    // Create and run the sink with mock data
    let sink = IpfsCacheSink::new(cache, ipfs_source);
    sink.run(StreamSource::mock()).await?;

    tracing::info!("Hermes IPFS Cache finished");

    Ok(())
}
