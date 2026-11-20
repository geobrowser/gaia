//! Hermes IPFS Cache
//!
//! Pre-populates resolved IPFS contents ahead of time so the edits transformer
//! doesn't block on network I/O.
//!
//! This service:
//! 1. Connects to hermes-substream `map_ipfs_uris` (parallelized, runs ahead)
//! 2. For each edit event, fetches the IPFS content by CID
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
//! let mut data = HashMap::new();
//! data.insert("QmTestCid".to_string(), grc20_bytes);
//! let sink = IpfsCacheSink::new(cache, IpfsSource::mock_bytes(data));
//! sink.run(StreamSource::mock()).await?;
//!
//! // Production: use live sources
//! let cache = CacheSource::live(&database_url).into_cache().await?;
//! let sink = IpfsCacheSink::new(cache, IpfsSource::live(&gateway_url));
//! sink.run(StreamSource::live(&endpoint, module, start, end)).await?;
//! ```

pub mod backfill;
pub mod cache;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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
///
/// Invariant (GEO-3803): a URI holds its permit for at most the ipfs client's
/// total budget (180 s, `IpfsClient::DEFAULT_TOTAL_BUDGET`). Permits are taken
/// in block order and tokio's `Semaphore` is FIFO, so once hermes-pipeline
/// reaches a block, that block's URIs queue only behind their own siblings,
/// and the block completes within `ceil(uris / 20) x 180 s`. Keep that under
/// hermes-pipeline's `IPFS_WARMER_WAIT_SECS` (600 s): up to 60 URIs per block
/// (recent blocks carry 1 to 3). Past it the pipeline drops the edits as
/// missing rows, which `reconcile_errored` cannot see.
const MAX_CONCURRENT_FETCHES: usize = 20;

/// Counter of cache-write retries (a `put` failed and is being attempted again).
pub const CACHE_WRITE_RETRIES: &str = "hermes_ipfs_cache_write_retries_total";

/// Counter of blocks held back because a cache write still failed after every retry.
pub const BLOCKS_HELD: &str = "hermes_ipfs_cache_blocks_held_total";

/// Bounded retry policy for cache writes (`attempts` tries, delay doubling from `base`).
#[derive(Debug, Clone, Copy)]
pub struct WriteRetry {
    pub attempts: u32,
    pub base: Duration,
    pub max_delay: Duration,
}

impl Default for WriteRetry {
    /// 5 tries, waits of 0.5 s, 1 s, 2 s, 4 s: about 7.5 s of waiting, long enough for a
    /// saturated pool (3 s acquire timeout) to drain.
    fn default() -> Self {
        Self {
            attempts: 5,
            base: Duration::from_millis(500),
            max_delay: Duration::from_secs(5),
        }
    }
}

/// Error type for the IPFS cache sink.
#[derive(Debug, thiserror::Error)]
pub enum IpfsCacheError {
    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// A cache write failed for good. The block stays pending so its cursor is never
    /// persisted; stopping lets the process restart from the last persisted cursor.
    #[error(
        "cache write failed for block {0}; stopping so it is re-processed from the persisted cursor"
    )]
    BlockHeld(u64),
}

/// Tracks pending fetches per block for cursor management.
///
/// We only persist the cursor when a block fully completes (all fetches done).
/// This ensures correctness while maintaining parallelism across blocks.
#[derive(Default)]
struct PendingFetches {
    /// Map of block number -> (cursor, block timestamp seconds, pending count, total count, started at)
    blocks: BTreeMap<u64, (String, i64, usize, usize, Instant)>,
}

impl PendingFetches {
    /// Register pending fetches for a block.
    fn add_block(&mut self, block: u64, cursor: String, timestamp_seconds: i64, count: usize) {
        if count > 0 {
            self.blocks.insert(
                block,
                (cursor, timestamp_seconds, count, count, Instant::now()),
            );
        }
    }

    /// Mark one fetch as complete for a block.
    ///
    /// Returns `Some((block, cursor, timestamp_seconds, total, duration))` if
    /// this block is now fully complete AND it's the minimum block (safe to
    /// persist).
    fn complete_one(&mut self, block: u64) -> Option<(u64, String, i64, usize, Duration)> {
        // First check if this block exists and decrement
        let (is_complete, cursor, timestamp_seconds, total, started_at) =
            match self.blocks.get_mut(&block) {
                Some((cursor, timestamp_seconds, remaining, total, started_at)) => {
                    *remaining = remaining.saturating_sub(1);
                    (
                        *remaining == 0,
                        cursor.clone(),
                        *timestamp_seconds,
                        *total,
                        *started_at,
                    )
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
            Some((
                block,
                cursor,
                timestamp_seconds,
                total,
                started_at.elapsed(),
            ))
        } else {
            None
        }
    }
}

/// IPFS cache sink that implements the hermes-relay Sink trait.
///
/// Subscribes to `map_ipfs_uris` (edits plus created/updated proposal content) and pre-fetches IPFS content
/// to populate the cache for downstream consumers.
pub struct IpfsCacheSink {
    cache: Arc<Mutex<Cache>>,
    ipfs: Arc<dyn IpfsFetcher>,
    semaphore: Arc<Semaphore>,
    pending: Arc<Mutex<PendingFetches>>,
    /// Best-effort geo-db mirror of the cursor; `None` unless `GEODB_CURSOR_MIRROR` is on.
    geodb_mirror: Option<Arc<geodb_cursor::CursorMirror>>,
    write_retry: WriteRetry,
    /// Lowest block whose cache write failed after all retries, or `u64::MAX` for none.
    held_block: Arc<AtomicU64>,
}

impl IpfsCacheSink {
    /// Create a new IPFS cache sink with the given IPFS source configuration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Development: use mock IPFS data
    /// let mut data = HashMap::new();
    /// data.insert("QmTestCid".to_string(), grc20_bytes);
    /// let sink = IpfsCacheSink::new(cache, IpfsSource::mock_bytes(data));
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
            geodb_mirror: geodb_cursor::CursorMirror::from_env().map(Arc::new),
            write_retry: WriteRetry::default(),
            held_block: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }

    /// Override the cache-write retry policy (tests).
    pub fn with_write_retry(mut self, retry: WriteRetry) -> Self {
        self.write_retry = retry;
        self
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
        // A block whose cache write failed is still pending, so its cursor cannot persist.
        // Stop instead of running on: the restart resumes from the cursor behind it.
        let held = self.held_block.load(Ordering::SeqCst);
        if held != u64::MAX {
            return Err(IpfsCacheError::BlockHeld(held));
        }

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
        let block_timestamp_seconds: i64 = data
            .clock
            .as_ref()
            .and_then(|c| c.timestamp.as_ref())
            .map(|t| t.seconds)
            .unwrap_or(0);
        let cursor = data.cursor.clone();
        let edit_count = uri_list.uris.len();

        // Checkpoint log every 100 blocks
        if block_number.is_multiple_of(100) {
            info!(block = block_number, "Checkpoint");
        }

        // Skip empty blocks entirely - no span created.
        // Still advance the lag gauges so HermesBehindChainTip doesn't false-fire
        // during stretches of blocks that happen to contain no IPFS URIs.
        if edit_count == 0 {
            hermes_instrumentation::metrics::set_latest_processed_block(block_number);
            hermes_instrumentation::metrics::set_latest_processed_block_timestamp(
                block_timestamp_seconds,
            );
            return Ok(());
        }

        // Create root span for this block - this becomes the transaction in Sentry.
        // Child spans created while this span is entered will have it as their parent.
        let block_span = info_span!(
            "ipfs_cache.process_block",
            block_number = block_number,
            edit_count = edit_count,
            "otel.status_code" = tracing::field::Empty,
            "otel.status_message" = tracing::field::Empty
        );
        let _block_guard = block_span.enter();

        info!(
            event = "ipfs_cache.batch_start",
            block_number = block_number,
            edit_count = edit_count,
            "Batch start"
        );
        info!(block = block_number, edits = edit_count, "Processing edits");

        // Register all pending fetches for this block upfront
        self.pending.lock().await.add_block(
            block_number,
            cursor.clone(),
            block_timestamp_seconds,
            edit_count,
        );

        // Process each IPFS URI
        for ipfs_uri in uri_list.uris {
            let permit = self.semaphore.clone().acquire_owned().await.unwrap();
            let cache = self.cache.clone();
            let ipfs = self.ipfs.clone();
            let pending = self.pending.clone();
            let geodb_mirror = self.geodb_mirror.clone();
            let write_retry = self.write_retry;
            let held_block = self.held_block.clone();
            let block_str = block_number.to_string();
            let block_num = block_number;

            let ipfs_hash = ipfs_uri
                .uri
                .strip_prefix("ipfs://")
                .unwrap_or("")
                .to_string();
            let space_id = hex::encode(&ipfs_uri.space_id);
            let span = info_span!(
                "ipfs_cache.fetch_uri",
                block_number = block_num,
                space_id = %space_id,
                uri = %ipfs_uri.uri,
                ipfs_hash = %ipfs_hash,
                source = %ipfs_uri.source
            );
            task::spawn(
                async move {
                    let uri = ipfs_uri.uri.clone();
                    let ipfs_hash_for_log = ipfs_hash.clone();
                    let result = process_ipfs_uri(
                        ipfs_uri,
                        &cache,
                        &ipfs,
                        &block_str,
                        block_num,
                        write_retry,
                    )
                    .await;
                    if let Err(e) = result {
                        // The cache write failed after every retry. Real IPFS and decode
                        // failures never land here (they are stored as is_errored rows).
                        // Do NOT complete the block: it stays pending, so its cursor (and
                        // every later one) is never persisted and a restart re-processes it.
                        metrics::counter!(BLOCKS_HELD).increment(1);
                        held_block.fetch_min(block_num, Ordering::SeqCst);
                        error!(
                            event = "ipfs_cache.block_held",
                            block_number = block_num,
                            space_id = %space_id,
                            uri = %uri,
                            error = %e,
                            "Cache write failed after retries; holding block cursor"
                        );
                        if ipfs_hash_for_log.is_empty() {
                            error!(
                                event = "ipfs_cache.fetch_failed",
                                block_number = block_num,
                                space_id = %space_id,
                                uri = %uri,
                                error = %e,
                                "Failed to process edit event"
                            );
                        } else {
                            error!(
                                event = "ipfs_cache.fetch_failed",
                                block_number = block_num,
                                space_id = %space_id,
                                uri = %uri,
                                ipfs_hash = %ipfs_hash_for_log,
                                tags.ipfs_hash = %ipfs_hash_for_log,
                                error = %e,
                                "Failed to process edit event"
                            );
                        }
                        drop(permit);
                        return;
                    }

                    // Mark this fetch as complete - persist cursor if block fully completed
                    let cursor_to_persist = pending.lock().await.complete_one(block_num);

                    if let Some((
                        persist_block,
                        persist_cursor,
                        persist_timestamp,
                        total,
                        duration,
                    )) = cursor_to_persist
                    {
                        debug!(
                            block = persist_block,
                            "Block fully cached, persisting cursor"
                        );
                        info!(
                            event = "ipfs_cache.batch_end",
                            indexer_id = INDEXER_ID,
                            block_number = persist_block,
                            cursor = %persist_cursor,
                            edit_count = total,
                            duration_ms = duration.as_millis(),
                            "Batch end"
                        );
                        if let Err(e) = cache
                            .lock()
                            .await
                            .persist_cursor(INDEXER_ID, &persist_cursor, persist_block)
                            .await
                        {
                            error!(
                                event = "ipfs_cache.persist_cursor_failed",
                                indexer_id = INDEXER_ID,
                                block_number = persist_block,
                                cursor = %persist_cursor,
                                error = %e,
                                "Failed to persist cursor"
                            );
                        } else {
                            if let Some(m) = &geodb_mirror {
                                m.mirror(INDEXER_ID, &persist_cursor, persist_block).await;
                            }
                            // Cursor durably persisted — update lag gauges.
                            hermes_instrumentation::metrics::set_latest_processed_block(
                                persist_block,
                            );
                            hermes_instrumentation::metrics::set_latest_processed_block_timestamp(
                                persist_timestamp,
                            );
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
/// The URI is pre-validated by hermes-substream (only valid IPFS URIs are included).
async fn process_ipfs_uri(
    ipfs_uri: IpfsUri,
    cache: &Arc<Mutex<Cache>>,
    ipfs: &Arc<dyn IpfsFetcher>,
    block_str: &str,
    block_number: u64,
    write_retry: WriteRetry,
) -> Result<(), CacheError> {
    let uri = ipfs_uri.uri;
    let ipfs_hash = uri.strip_prefix("ipfs://").unwrap_or("").to_string();
    let space_id = hex::encode(&ipfs_uri.space_id);

    debug!(
        uri = %uri,
        ipfs_hash = %ipfs_hash,
        space_id = %space_id,
        block = block_number,
        "Processing edit"
    );

    // A warmer restart replays from the last persisted cursor and re-fetches
    // URIs that already have a row. `put` is first-write-wins, so fetching
    // again cannot change anything; skip it. A failed lookup counts as "not
    // cached": returning the error here would complete the block with no row
    // at all.
    match cache.lock().await.get(&uri).await {
        Ok(Some(_)) => {
            debug!(uri = %uri, block = block_number, "Already cached; skipping fetch");
            return Ok(());
        }
        Ok(None) => {}
        Err(e) => warn!(uri = %uri, error = %e, "Cache lookup failed; fetching anyway"),
    }

    // Fetch the raw bytes from IPFS. The client retries transient failures
    // (timeouts, 404, 429, 5xx, 401/403) for up to its 180 s budget; what comes back
    // here is final for this pass.
    let result = ipfs.get_bytes(&uri).await;

    let item = match result {
        Ok(bytes) => {
            // Validate by decoding with grc-20 (supports GRC2 and GRC2Z formats)
            match decode_edit(&bytes) {
                Ok(edit) => {
                    // Extract the edit name (convert from Cow to owned String)
                    let name = if edit.name.is_empty() {
                        None
                    } else {
                        Some(edit.name.into_owned())
                    };
                    // Valid GRC-20 v2 format - store raw bytes and name
                    info!(
                        uri = %uri,
                        ipfs_hash = %ipfs_hash,
                        block = block_number,
                        bytes = bytes.len(),
                        name = ?name,
                        "Cached IPFS content (GRC-20 v2 validated)"
                    );
                    CacheItem {
                        uri,
                        data: Some(bytes),
                        block: block_str.to_string(),
                        space_id,
                        is_errored: false,
                        name,
                    }
                }
                Err(decode_error) => {
                    // Invalid GRC-20 format - mark as errored
                    warn!(
                        uri = %uri,
                        ipfs_hash = %ipfs_hash,
                        tags.ipfs_hash = %ipfs_hash,
                        space_id = %space_id,
                        block = block_number,
                        bytes = bytes.len(),
                        error = %decode_error,
                        "Invalid GRC-20 v2 format"
                    );
                    CacheItem {
                        uri,
                        data: None,
                        block: block_str.to_string(),
                        space_id,
                        is_errored: true,
                        name: None,
                    }
                }
            }
        }
        Err(error) => {
            // `transient` = the gateway, not the content, failed for the whole
            // budget: a candidate for reconcile_errored once it recovers.
            let transient = error.is_transient();
            if ipfs_hash.is_empty() {
                warn!(
                    uri = %uri,
                    space_id = %space_id,
                    block = block_number,
                    error = %error,
                    transient,
                    "Failed to fetch IPFS content; caching as errored"
                );
            } else {
                warn!(
                    uri = %uri,
                    ipfs_hash = %ipfs_hash,
                    tags.ipfs_hash = %ipfs_hash,
                    space_id = %space_id,
                    block = block_number,
                    error = %error,
                    transient,
                    "Failed to fetch IPFS content; caching as errored"
                );
            }
            CacheItem {
                uri,
                data: None,
                block: block_str.to_string(),
                space_id,
                is_errored: true,
                name: None,
            }
        }
    };

    put_with_retry(cache, &item, write_retry).await
}

/// Write `item`, retrying with doubling backoff. Returns the last error once the attempts
/// are spent. A pool timeout under load is the common transient failure.
async fn put_with_retry(
    cache: &Arc<Mutex<Cache>>,
    item: &CacheItem,
    retry: WriteRetry,
) -> Result<(), CacheError> {
    let mut delay = retry.base;
    let mut attempt = 1;
    loop {
        // Take the lock per attempt, so the backoff sleep does not stall other writers.
        match cache.lock().await.put(item).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= retry.attempts => return Err(e),
            Err(e) => {
                metrics::counter!(CACHE_WRITE_RETRIES).increment(1);
                warn!(
                    event = "ipfs_cache.write_retry",
                    uri = %item.uri,
                    block = %item.block,
                    attempt = attempt,
                    error = %e,
                    "Cache write failed; retrying"
                );
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(retry.max_delay);
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic Unix-seconds timestamps for tests. Distinct from block numbers
    /// so a regression that swaps the two would surface.
    const TS_100: i64 = 1_700_000_100;
    const TS_101: i64 = 1_700_000_101;
    const TS_102: i64 = 1_700_000_102;

    #[test]
    fn pending_fetches_single_block_single_edit() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 1);

        // Completing the only fetch should return the cursor
        let result = pending.complete_one(100);
        assert!(matches!(
            result,
            Some((100, ref cursor, TS_100, 1, _)) if cursor == "cursor_100"
        ));

        // No more pending
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_single_block_multiple_edits() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 3);

        // First two completions should not persist
        assert_eq!(pending.complete_one(100), None);
        assert_eq!(pending.complete_one(100), None);

        // Third completion should persist
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 3, _)) if cursor == "cursor_100"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_multiple_blocks_complete_in_order() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 2);
        pending.add_block(101, "cursor_101".to_string(), TS_101, 1);

        // Complete block 100 first
        assert_eq!(pending.complete_one(100), None); // 1 remaining
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 2, _)) if cursor == "cursor_100"
        ));

        // Now complete block 101
        assert!(matches!(
            pending.complete_one(101),
            Some((101, ref cursor, TS_101, 1, _)) if cursor == "cursor_101"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_multiple_blocks_complete_out_of_order() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 2);
        pending.add_block(101, "cursor_101".to_string(), TS_101, 1);

        // Complete block 101 first - should NOT persist (100 still pending)
        assert_eq!(pending.complete_one(101), None);

        // Block 101 is removed but we didn't get a cursor to persist
        assert!(!pending.blocks.contains_key(&101));

        // Complete block 100
        assert_eq!(pending.complete_one(100), None); // 1 remaining
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 2, _)) if cursor == "cursor_100"
        ));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_three_blocks_middle_completes_first() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 1);
        pending.add_block(101, "cursor_101".to_string(), TS_101, 1);
        pending.add_block(102, "cursor_102".to_string(), TS_102, 1);

        // Complete middle block - should NOT persist
        assert_eq!(pending.complete_one(101), None);

        // Complete last block - should NOT persist (100 still pending)
        assert_eq!(pending.complete_one(102), None);

        // Complete first block - should persist
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 1, _)) if cursor == "cursor_100"
        ));

        // 101 and 102 were already removed, nothing left
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_empty_block_not_added() {
        let mut pending = PendingFetches::default();

        // Adding a block with 0 edits should not add it
        pending.add_block(100, "cursor_100".to_string(), TS_100, 0);

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_complete_unknown_block() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 1);

        // Completing an unknown block should return None
        assert_eq!(pending.complete_one(999), None);

        // Original block still pending
        assert_eq!(pending.blocks.len(), 1);
    }

    #[test]
    fn pending_fetches_later_blocks_complete_then_first() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 1);
        pending.add_block(101, "cursor_101".to_string(), TS_101, 1);
        pending.add_block(102, "cursor_102".to_string(), TS_102, 1);

        // Complete 102 first - no persist (100 still pending)
        assert_eq!(pending.complete_one(102), None);
        assert!(!pending.blocks.contains_key(&102));

        // Complete 101 - no persist (100 still pending)
        assert_eq!(pending.complete_one(101), None);
        assert!(!pending.blocks.contains_key(&101));

        // Complete 100 - persist cursor 100 (it's now the min and complete)
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 1, _)) if cursor == "cursor_100"
        ));

        // All blocks removed
        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_interleaved_completions() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), TS_100, 3);
        pending.add_block(101, "cursor_101".to_string(), TS_101, 2);

        // Interleaved completions
        assert_eq!(pending.complete_one(100), None); // 100: 2 remaining
        assert_eq!(pending.complete_one(101), None); // 101: 1 remaining
        assert_eq!(pending.complete_one(100), None); // 100: 1 remaining
        assert_eq!(pending.complete_one(101), None); // 101: 0 remaining, but 100 still pending
        assert!(matches!(
            pending.complete_one(100),
            Some((100, ref cursor, TS_100, 3, _)) if cursor == "cursor_100"
        )); // 100: 0 remaining

        assert!(pending.blocks.is_empty());
    }

    // ---- GEO-3803: what the warmer writes after the client's retries --------

    /// Fails with each queued error in turn, then serves `bytes`.
    struct ScriptedFetcher {
        failures: std::sync::Mutex<std::collections::VecDeque<ipfs::IpfsError>>,
        bytes: Vec<u8>,
        calls: std::sync::atomic::AtomicU32,
    }

    #[async_trait::async_trait]
    impl IpfsFetcher for ScriptedFetcher {
        async fn get_bytes(&self, _uri: &str) -> ipfs::Result<Vec<u8>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.failures.lock().unwrap().pop_front() {
                Some(err) => Err(err),
                None => Ok(self.bytes.clone()),
            }
        }
    }

    fn scripted(failures: Vec<ipfs::IpfsError>) -> Arc<ScriptedFetcher> {
        let edit = grc_20::Edit {
            id: [0x01; 16],
            name: std::borrow::Cow::Borrowed("edit"),
            authors: vec![[0x02; 16]],
            created_at: 1_700_000_000,
            ops: vec![],
        };
        Arc::new(ScriptedFetcher {
            failures: std::sync::Mutex::new(failures.into()),
            bytes: grc_20::encode_edit(&edit).unwrap(),
            calls: Default::default(),
        })
    }

    const URI: &str = "ipfs://bafkreiwarmer";

    async fn run(cache: &Arc<Mutex<Cache>>, fetcher: &Arc<ScriptedFetcher>) {
        let ipfs: Arc<dyn IpfsFetcher> = fetcher.clone();
        process_ipfs_uri(
            IpfsUri {
                uri: URI.to_string(),
                space_id: vec![0xaa; 16],
                source: "edit".to_string(),
            },
            cache,
            &ipfs,
            "100",
            100,
            WriteRetry::default(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_fetch_error_after_the_clients_retries_is_cached_as_errored() {
        for err in [
            ipfs::IpfsError::HttpStatus {
                status: 503,
                body: String::new(),
            },
            ipfs::IpfsError::VerificationFailed {
                cid: "bafkreiwarmer".into(),
                attempts: 15,
            },
        ] {
            let cache = Arc::new(Mutex::new(Cache::mock()));
            let fetcher = scripted(vec![err]);
            run(&cache, &fetcher).await;
            let item = cache.lock().await.get(URI).await.unwrap().unwrap();
            assert!(item.is_errored);
            assert_eq!(fetcher.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn fetched_content_is_cached() {
        let cache = Arc::new(Mutex::new(Cache::mock()));
        let fetcher = scripted(vec![]);
        run(&cache, &fetcher).await;
        let item = cache.lock().await.get(URI).await.unwrap().unwrap();
        assert!(!item.is_errored);
        assert!(item.data.is_some());
    }

    #[tokio::test]
    async fn an_already_cached_uri_is_not_fetched_again() {
        let cache = Arc::new(Mutex::new(Cache::mock()));
        cache
            .lock()
            .await
            .put(&CacheItem {
                uri: URI.to_string(),
                data: Some(vec![1, 2, 3]),
                block: "90".to_string(),
                space_id: hex::encode([0xaa; 16]),
                is_errored: false,
                name: None,
            })
            .await
            .unwrap();
        let fetcher = scripted(vec![]);
        run(&cache, &fetcher).await;
        assert_eq!(fetcher.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let item = cache.lock().await.get(URI).await.unwrap().unwrap();
        assert_eq!(item.block, "90");
    }

    // ---- cache-write failure handling ----

    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    use cache::{CacheStorage, MockStorage};
    use hermes_relay::source::MockSource;

    /// Storage whose `insert` fails the first `fail_first` calls (`usize::MAX`: always),
    /// like a pool timeout. Everything else delegates to an in-memory store.
    struct FlakyStorage {
        inner: MockStorage,
        fail_first: usize,
        inserts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CacheStorage for FlakyStorage {
        async fn insert(&self, item: &CacheItem) -> Result<(), CacheError> {
            let n = self.inserts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                return Err(CacheError::Database(sqlx::Error::PoolTimedOut));
            }
            self.inner.insert(item).await
        }
        async fn get(&self, uri: &str) -> Result<Option<CacheItem>, CacheError> {
            self.inner.get(uri).await
        }
        async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError> {
            self.inner.load_cursor(id).await
        }
        async fn persist_cursor(&self, id: &str, c: &str, b: u64) -> Result<(), CacheError> {
            self.inner.persist_cursor(id, c, b).await
        }
    }

    const CACHE_WRITE_URI: &str = "ipfs://QmX65gRctsCHnBZSG7izR4vhMUTH3HbMuCuHRYP8tu5BVv";

    fn fast_retry() -> WriteRetry {
        WriteRetry {
            attempts: 3,
            base: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
        }
    }

    fn sink_with(fail_first: usize) -> (IpfsCacheSink, Arc<AtomicUsize>) {
        let inserts = Arc::new(AtomicUsize::new(0));
        let storage = FlakyStorage {
            inner: MockStorage::new(),
            fail_first,
            inserts: inserts.clone(),
        };
        let mut data = HashMap::new();
        // Not valid GRC-20, so the item is stored as an errored row; the write path is
        // identical to the valid case.
        data.insert(URI.to_string(), vec![1u8, 2, 3]);
        let sink = IpfsCacheSink::new(
            Cache::with_storage(Box::new(storage)),
            IpfsSource::mock_bytes(data),
        )
        .with_write_retry(fast_retry());
        (sink, inserts)
    }

    fn block(n: u64) -> hermes_relay::stream::pb::sf::substreams::rpc::v2::BlockScopedData {
        let list = IpfsUriList {
            uris: vec![IpfsUri {
                uri: URI.to_string(),
                space_id: vec![7; 16],
                source: "edit".to_string(),
            }],
        };
        MockSource::builder(list.encode_to_vec())
            .single_block(n)
            .into_iter()
            .next()
            .unwrap()
    }

    /// Wait for the spawned fetch tasks to finish (all permits returned).
    async fn drain(sink: &IpfsCacheSink) {
        for _ in 0..500 {
            if sink.semaphore.available_permits() == MAX_CONCURRENT_FETCHES {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("fetch tasks did not finish");
    }

    #[tokio::test]
    async fn failed_cache_write_holds_the_block_and_its_cursor() {
        let (sink, inserts) = sink_with(usize::MAX);
        sink.process_block_scoped_data(&block(100)).await.unwrap();
        drain(&sink).await;

        // Every attempt was made, then the block was given up on without completing.
        assert_eq!(inserts.load(Ordering::SeqCst), 3);
        assert!(sink.pending.lock().await.blocks.contains_key(&100));
        assert_eq!(sink.load_persisted_cursor().await.unwrap(), None);
        assert!(sink.cache.lock().await.get(URI).await.unwrap().is_none());

        // A later block must not carry the cursor past the held one.
        sink.held_block.store(u64::MAX, Ordering::SeqCst);
        sink.process_block_scoped_data(&block(101)).await.unwrap();
        drain(&sink).await;
        assert_eq!(sink.load_persisted_cursor().await.unwrap(), None);
    }

    #[tokio::test]
    async fn held_block_stops_the_sink_so_it_restarts_behind_the_cursor() {
        let (sink, _) = sink_with(usize::MAX);
        sink.process_block_scoped_data(&block(100)).await.unwrap();
        drain(&sink).await;
        let err = sink
            .process_block_scoped_data(&block(101))
            .await
            .unwrap_err();
        assert!(matches!(err, IpfsCacheError::BlockHeld(100)));
    }

    #[tokio::test]
    async fn transient_cache_write_failure_is_retried_and_cursor_persists() {
        let (sink, inserts) = sink_with(2);
        sink.process_block_scoped_data(&block(100)).await.unwrap();
        drain(&sink).await;

        assert_eq!(inserts.load(Ordering::SeqCst), 3);
        assert!(sink.cache.lock().await.get(URI).await.unwrap().is_some());
        assert!(sink.pending.lock().await.blocks.is_empty());
        assert_eq!(
            sink.load_persisted_cursor().await.unwrap().as_deref(),
            Some("cursor_100")
        );
        // Not held: the next block is accepted.
        sink.process_block_scoped_data(&block(101)).await.unwrap();
    }
}
