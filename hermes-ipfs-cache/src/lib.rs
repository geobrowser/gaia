//! Hermes IPFS Cache
//!
//! Pre-populates resolved IPFS contents ahead of time so the edits transformer
//! doesn't block on network I/O.
//!
//! This service:
//! 1. Connects to hermes-substream `map_ipfs_uris` (parallelized, runs ahead)
//! 2. For each IPFS URI (from edits or proposals), fetches the content by CID
//! 3. Stores resolved content in the cache
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_ipfs_cache::{IpfsCacheSink, cache::CacheSource};
//! use hermes_relay::{Sink, StreamSource};
//! use ipfs::IpfsSource;
//! use std::collections::HashMap;
//!
//! // Development: use all mock sources
//! let cache = CacheSource::mock().into_cache().await?;
//! let mut edits = HashMap::new();
//! edits.insert("QmTestCid".to_string(), test_edit);
//! let sink = IpfsCacheSink::new(cache, IpfsSource::mock(edits));
//! sink.run(StreamSource::mock()).await?;
//!
//! // Production: use live sources
//! let cache = CacheSource::live(&database_url).into_cache().await?;
//! let sink = IpfsCacheSink::new(cache, IpfsSource::live(&gateway_url));
//! sink.run(StreamSource::live(&endpoint, module, start, end)).await?;
//! ```

pub mod cache;

use std::collections::BTreeMap;
use std::sync::Arc;

use grc_20::decode_edit;
use hermes_instrumentation::{debug, error, info, info_span, warn};
use hermes_relay::{HermesModule, Sink};
use hermes_substream::pb::hermes::{IpfsUri, IpfsUriList};
use ipfs::{IpfsFetcher, IpfsSource};
use prost::Message;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};
use tokio::task;
use tracing::Instrument;

use cache::{Cache, CacheError, CacheItem};

/// Indexer ID for cursor persistence.
const INDEXER_ID: &str = "hermes_ipfs_cache";

/// Maximum concurrent IPFS fetches.
const MAX_CONCURRENT_FETCHES: usize = 20;

/// Error type for the IPFS cache sink.
#[derive(Debug, thiserror::Error)]
pub enum IpfsCacheError {
    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Tracks pending fetches per block for cursor management.
///
/// We only persist the cursor when a block fully completes (all fetches done).
/// This ensures correctness while maintaining parallelism across blocks.
#[derive(Default)]
struct PendingFetches {
    /// Map of block number -> (cursor, pending count)
    blocks: BTreeMap<u64, (String, usize, usize, Instant)>,
}

impl PendingFetches {
    /// Register pending fetches for a block.
    fn add_block(&mut self, block: u64, cursor: String, count: usize) {
        if count > 0 {
            self.blocks
                .insert(block, (cursor, count, count, Instant::now()));
        }
    }

    /// Mark one fetch as complete for a block.
    ///
    /// Returns `Some((block, cursor))` if this block is now fully complete
    /// AND it's the minimum block (safe to persist).
    fn complete_one(&mut self, block: u64) -> Option<(u64, String, usize, Duration)> {
        // First check if this block exists and decrement
        let (is_complete, cursor, total, started_at) = match self.blocks.get_mut(&block) {
            Some((cursor, remaining, total, started_at)) => {
                *remaining = remaining.saturating_sub(1);
                (*remaining == 0, cursor.clone(), *total, *started_at)
            }
            None => return None,
        };

        if !is_complete {
            return None;
        }

        // Block is complete - check if it's the minimum before removing
        let is_min = self.blocks.first_key_value().map(|(b, _)| *b) == Some(block);
        self.blocks.remove(&block);

        if is_min {
            Some((block, cursor, total, started_at.elapsed()))
        } else {
            None
        }
    }
}

/// IPFS cache sink that implements the hermes-relay Sink trait.
///
/// Subscribes to `EditsPublished` events and pre-fetches IPFS content
/// to populate the cache for downstream consumers.
pub struct IpfsCacheSink {
    cache: Arc<Mutex<Cache>>,
    ipfs: Arc<dyn IpfsFetcher>,
    semaphore: Arc<Semaphore>,
    pending: Arc<Mutex<PendingFetches>>,
}

impl IpfsCacheSink {
    /// Create a new IPFS cache sink with the given IPFS source configuration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Development: use mock IPFS data
    /// let mut edits = HashMap::new();
    /// edits.insert("QmTestCid".to_string(), test_edit);
    /// let sink = IpfsCacheSink::new(cache, IpfsSource::mock(edits));
    ///
    /// // Production: use live IPFS gateway
    /// let sink = IpfsCacheSink::new(cache, IpfsSource::live("https://ipfs.io/ipfs/"));
    /// ```
    pub fn new(cache: Cache, ipfs_source: IpfsSource) -> Self {
        Self {
            cache: Arc::new(Mutex::new(cache)),
            ipfs: Arc::from(ipfs_source.into_fetcher()),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            pending: Arc::new(Mutex::new(PendingFetches::default())),
        }
    }

    /// Get the hermes module this sink subscribes to.
    pub fn module() -> HermesModule {
        HermesModule::IpfsUris
    }
}

impl Sink for IpfsCacheSink {
    type Error = IpfsCacheError;

    async fn process_block_scoped_data(
        &self,
        data: &hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData,
    ) -> Result<(), Self::Error> {
        // Get the output from the block data
        let output = data
            .output
            .as_ref()
            .and_then(|o| o.map_output.as_ref())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing map output")
            })?;

        // Decode the IpfsUriList from the output
        let uri_list = IpfsUriList::decode(output.value.as_slice())?;

        // Get block metadata
        let block_number = data.clock.as_ref().map(|c| c.number).unwrap_or(0);
        let cursor = data.cursor.clone();

        let block_timestamp = data
            .clock
            .as_ref()
            .and_then(|c| c.timestamp.as_ref())
            .map(|t| t.seconds.to_string())
            .unwrap_or_default();

        let uri_count = uri_list.uris.len();

        // Checkpoint log every 100 blocks
        if block_number.is_multiple_of(100) {
            info!(block = block_number, "Checkpoint");
        }

        if uri_count > 0 {
            info!(
                event = "ipfs_cache.batch_start",
                block_number = block_number,
                cursor = %cursor,
                uri_count = uri_count,
                "Batch start"
            );
            info!(
                block = block_number,
                uris = uri_count,
                "Processing IPFS URIs"
            );

            // Register all pending fetches for this block upfront
            self.pending
                .lock()
                .await
                .add_block(block_number, cursor.clone(), uri_count);
        }

        // Process each IPFS URI
        for ipfs_uri in uri_list.uris {
            let permit = self.semaphore.clone().acquire_owned().await.unwrap();
            let cache = self.cache.clone();
            let ipfs_client = self.ipfs.clone();
            let pending = self.pending.clone();
            let block_ts = block_timestamp.clone();
            let block_num = block_number;

            let ipfs_hash = ipfs_uri
                .uri
                .strip_prefix("ipfs://")
                .unwrap_or("")
                .to_string();
            let span = info_span!(
                "ipfs_cache.fetch",
                block_number = block_num,
                uri = %ipfs_uri.uri,
                source = %ipfs_uri.source,
                ipfs_hash = %ipfs_hash
            );
            task::spawn(
                async move {
                    let uri = ipfs_uri.uri.clone();
                    let ipfs_hash_for_log = ipfs_hash.clone();
                    let result =
                        process_ipfs_uri(ipfs_uri, &cache, &ipfs_client, &block_ts, block_num)
                            .await;
                    if let Err(e) = result {
                        if ipfs_hash_for_log.is_empty() {
                            error!(
                                event = "ipfs_cache.fetch_failed",
                                block_number = block_num,
                                uri = %uri,
                                error = %e,
                                "Failed to fetch IPFS content"
                            );
                        } else {
                            error!(
                                event = "ipfs_cache.fetch_failed",
                                block_number = block_num,
                                uri = %uri,
                                ipfs_hash = %ipfs_hash_for_log,
                                tags.ipfs_hash = %ipfs_hash_for_log,
                                error = %e,
                                "Failed to fetch IPFS content"
                            );
                        }
                    }

                    // Mark this fetch as complete - persist cursor if block fully completed
                    let cursor_to_persist = pending.lock().await.complete_one(block_num);

                    if let Some((persist_block, persist_cursor, total, duration)) =
                        cursor_to_persist
                    {
                        debug!(
                            block = persist_block,
                            "Block fully cached, persisting cursor"
                        );
                        info!(
                            event = "ipfs_cache.batch_end",
                            block_number = persist_block,
                            cursor = %persist_cursor,
                            uri_count = total,
                            duration_ms = duration.as_millis(),
                            "Batch end"
                        );
                        if let Err(e) = cache
                            .lock()
                            .await
                            .persist_cursor(INDEXER_ID, &persist_cursor, persist_block)
                            .await
                        {
                            error!(error = %e, "Failed to persist cursor");
                        }
                    }

                    drop(permit);
                }
                .instrument(span),
            );
        }

        Ok(())
    }

    async fn persist_cursor(&self, _cursor: String, _block: u64) -> Result<(), Self::Error> {
        // No-op: cursor persistence is handled in the spawned tasks after each fetch completes.
        // This ensures we only persist the cursor for the minimum pending block.
        Ok(())
    }

    async fn load_persisted_cursor(&self) -> Result<Option<String>, Self::Error> {
        let cursor = self.cache.lock().await.load_cursor(INDEXER_ID).await?;
        match &cursor {
            Some(c) => info!(cursor = %c, "Resuming from persisted cursor"),
            None => info!("No persisted cursor found, starting fresh"),
        }
        Ok(cursor)
    }
}

/// Process a single IPFS URI by fetching its content.
///
/// The `uri` field is pre-validated by hermes-substream's `map_ipfs_uris`.
/// URIs come from both EDITS_PUBLISHED and PROPOSAL_CREATED (Publish actions).
async fn process_ipfs_uri(
    ipfs_uri: IpfsUri,
    cache: &Arc<Mutex<Cache>>,
    ipfs_client: &Arc<dyn IpfsFetcher>,
    block_timestamp: &str,
    block_number: u64,
) -> Result<(), CacheError> {
    // URI is pre-validated by hermes-substream - should never be empty
    if ipfs_uri.uri.is_empty() {
        let space_id = hex::encode(&ipfs_uri.space_id);
        warn!(
            space_id = %space_id,
            source = %ipfs_uri.source,
            block = block_number,
            "IPFS URI is empty (should not happen)"
        );
        return Ok(());
    }

    let uri = ipfs_uri.uri;
    let ipfs_hash = uri.strip_prefix("ipfs://").unwrap_or("").to_string();
    let space_id = hex::encode(&ipfs_uri.space_id);
    let source = ipfs_uri.source;

    debug!(
        uri = %uri,
        ipfs_hash = %ipfs_hash,
        space_id = %space_id,
        source = %source,
        block = block_number,
        "Processing IPFS URI"
    );

    // Fetch the raw bytes from IPFS
    let result = ipfs_client.get_bytes(&uri).await;

    let item = match result {
        Ok(bytes) => {
            // Validate by decoding with grc-20 (supports GRC2 and GRC2Z formats)
            match decode_edit(&bytes) {
                Ok(_edit) => {
                    // Valid GRC-20 v2 format - store raw bytes
                    info!(
                        uri = %uri,
                        ipfs_hash = %ipfs_hash,
                        source = %source,
                        block = block_number,
                        bytes = bytes.len(),
                        "Cached IPFS content (GRC-20 v2 validated)"
                    );
                    CacheItem {
                        uri,
                        data: Some(bytes),
                        block: block_timestamp.to_string(),
                        space_id,
                        is_errored: false,
                    }
                }
                Err(decode_error) => {
                    // Invalid GRC-20 format - mark as errored
                    warn!(
                        uri = %uri,
                        ipfs_hash = %ipfs_hash,
                        tags.ipfs_hash = %ipfs_hash,
                        source = %source,
                        block = block_number,
                        bytes = bytes.len(),
                        error = %decode_error,
                        "Invalid GRC-20 v2 format"
                    );
                    CacheItem {
                        uri,
                        data: None,
                        block: block_timestamp.to_string(),
                        space_id,
                        is_errored: true,
                    }
                }
            }
        }
        Err(error) => {
            if ipfs_hash.is_empty() {
                warn!(
                    uri = %uri,
                    source = %source,
                    block = block_number,
                    error = %error,
                    "Failed to fetch IPFS content"
                );
            } else {
                warn!(
                    uri = %uri,
                    ipfs_hash = %ipfs_hash,
                    tags.ipfs_hash = %ipfs_hash,
                    source = %source,
                    block = block_number,
                    error = %error,
                    "Failed to fetch IPFS content"
                );
            }
            CacheItem {
                uri,
                data: None,
                block: block_timestamp.to_string(),
                space_id,
                is_errored: true,
            }
        }
    };

    cache.lock().await.put(&item).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_fetches_single_block_single_edit() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 1);

        // Completing the only fetch should return the cursor
        let result = pending.complete_one(100);
        assert!(matches!(
            result,
            Some((100, ref cursor, 1, _)) if cursor == "cursor_100"
        ));

        // No more pending
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_single_block_multiple_edits() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 3);

        // First two completions should not persist
        assert_eq!(pending.complete_one(100), None);
        assert_eq!(pending.complete_one(100), None);

        // Third completion should persist
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 3, _)) if cursor == "cursor_100"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_multiple_blocks_complete_in_order() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 2);
        pending.add_block(101, "cursor_101".to_string(), 1);

        // Complete block 100 first
        assert_eq!(pending.complete_one(100), None); // 1 remaining
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 2, _)) if cursor == "cursor_100"
        ));

        // Now complete block 101
        assert!(matches!(
            pending.complete_one(101),
            Some((101, ref cursor, 1, _)) if cursor == "cursor_101"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_multiple_blocks_complete_out_of_order() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 2);
        pending.add_block(101, "cursor_101".to_string(), 1);

        // Complete block 101 first - should NOT persist (100 still pending)
        assert_eq!(pending.complete_one(101), None);

        // Block 101 is removed but we didn't get a cursor to persist
        assert!(!pending.blocks.contains_key(&101));

        // Complete block 100
        assert_eq!(pending.complete_one(100), None); // 1 remaining
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 2, _)) if cursor == "cursor_100"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_three_blocks_middle_completes_first() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 1);
        pending.add_block(101, "cursor_101".to_string(), 1);
        pending.add_block(102, "cursor_102".to_string(), 1);

        // Complete middle block - should NOT persist
        assert_eq!(pending.complete_one(101), None);

        // Complete last block - should NOT persist (100 still pending)
        assert_eq!(pending.complete_one(102), None);

        // Complete first block - should persist
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 1, _)) if cursor == "cursor_100"
        ));

        // 101 and 102 were already removed, nothing left
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_empty_block_not_added() {
        let mut pending = PendingFetches::default();

        // Adding a block with 0 edits should not add it
        pending.add_block(100, "cursor_100".to_string(), 0);

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_complete_unknown_block() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 1);

        // Completing an unknown block should return None
        assert_eq!(pending.complete_one(999), None);

        // Original block still pending
        assert_eq!(pending.blocks.len(), 1);
    }

    #[test]
    fn pending_fetches_later_blocks_complete_then_first() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 1);
        pending.add_block(101, "cursor_101".to_string(), 1);
        pending.add_block(102, "cursor_102".to_string(), 1);

        // Complete 102 first - no persist (100 still pending)
        assert_eq!(pending.complete_one(102), None);
        assert!(!pending.blocks.contains_key(&102));

        // Complete 101 - no persist (100 still pending)
        assert_eq!(pending.complete_one(101), None);
        assert!(!pending.blocks.contains_key(&101));

        // Complete 100 - persist cursor 100 (it's now the min and complete)
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 1, _)) if cursor == "cursor_100"
        ));

        // All blocks removed
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_interleaved_completions() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 3);
        pending.add_block(101, "cursor_101".to_string(), 2);

        // Interleaved completions
        assert_eq!(pending.complete_one(100), None); // 100: 2 remaining
        assert_eq!(pending.complete_one(101), None); // 101: 1 remaining
        assert_eq!(pending.complete_one(100), None); // 100: 1 remaining
        assert_eq!(pending.complete_one(101), None); // 101: 0 remaining, but 100 still pending
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, 3, _)) if cursor == "cursor_100"
        )); // 100: 0 remaining

        assert!(pending.blocks.is_empty());
    }
}
