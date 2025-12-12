//! Hermes IPFS Cache
//!
//! Pre-populates resolved IPFS contents ahead of time so the edits transformer
//! doesn't block on network I/O.
//!
//! This service:
//! 1. Connects to hermes-substream `map_edits_published` (parallelized, runs ahead)
//! 2. For each edit event, fetches the IPFS content by CID
//! 3. Stores resolved content in the cache
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_ipfs_cache::{IpfsCacheSink, cache::Cache};
//!
//! let cache = Cache::new(storage);
//! let ipfs = ipfs::IpfsClient::new(&gateway_url);
//! let sink = IpfsCacheSink::new(cache, ipfs);
//!
//! sink.run(
//!     &endpoint_url,
//!     HermesModule::EditsPublished,
//!     start_block,
//!     end_block,
//! ).await?;
//! ```

pub mod cache;

use std::collections::BTreeMap;
use std::sync::Arc;

use hermes_relay::{HermesModule, Sink};
use hermes_substream::pb::hermes::{EditsPublished, EditsPublishedList};
use ipfs::IpfsClient;
use prost::Message;
use tokio::sync::{Mutex, Semaphore};
use tokio::task;

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
    blocks: BTreeMap<u64, (String, usize)>,
}

impl PendingFetches {
    /// Register pending fetches for a block.
    fn add_block(&mut self, block: u64, cursor: String, count: usize) {
        if count > 0 {
            self.blocks.insert(block, (cursor, count));
        }
    }

    /// Mark one fetch as complete for a block.
    ///
    /// Returns `Some((block, cursor))` if this block is now fully complete
    /// AND it's the minimum block (safe to persist).
    fn complete_one(&mut self, block: u64) -> Option<(u64, String)> {
        // First check if this block exists and decrement
        let (is_complete, cursor) = match self.blocks.get_mut(&block) {
            Some((cursor, count)) => {
                *count = count.saturating_sub(1);
                (*count == 0, cursor.clone())
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
            Some((block, cursor))
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
    ipfs: Arc<IpfsClient>,
    semaphore: Arc<Semaphore>,
    pending: Arc<Mutex<PendingFetches>>,
}

impl IpfsCacheSink {
    /// Create a new IPFS cache sink.
    pub fn new(cache: Cache, ipfs: IpfsClient) -> Self {
        Self {
            cache: Arc::new(Mutex::new(cache)),
            ipfs: Arc::new(ipfs),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            pending: Arc::new(Mutex::new(PendingFetches::default())),
        }
    }

    /// Get the hermes module this sink subscribes to.
    pub fn module() -> HermesModule {
        HermesModule::EditsPublished
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

        // Decode the EditsPublishedList from the output
        let edits_list = EditsPublishedList::decode(output.value.as_slice())?;

        // Get block metadata
        let block_number = data.clock.as_ref().map(|c| c.number).unwrap_or(0);
        let cursor = data.cursor.clone();

        let block_timestamp = data
            .clock
            .as_ref()
            .and_then(|c| c.timestamp.as_ref())
            .map(|t| t.seconds.to_string())
            .unwrap_or_default();

        let edit_count = edits_list.edits.len();

        if edit_count > 0 {
            tracing::info!(
                block = block_number,
                edits = edit_count,
                "Processing edits"
            );

            // Register all pending fetches for this block upfront
            self.pending
                .lock()
                .await
                .add_block(block_number, cursor.clone(), edit_count);
        }

        // Process each edit event
        for edit in edits_list.edits {
            let permit = self.semaphore.clone().acquire_owned().await.unwrap();
            let cache = self.cache.clone();
            let ipfs = self.ipfs.clone();
            let pending = self.pending.clone();
            let block_ts = block_timestamp.clone();
            let block_num = block_number;

            task::spawn(async move {
                let result = process_edit_event(edit, &cache, &ipfs, &block_ts, block_num).await;
                if let Err(e) = result {
                    tracing::error!(error = %e, "Failed to process edit event");
                }

                // Mark this fetch as complete - persist cursor if block fully completed
                let cursor_to_persist = pending.lock().await.complete_one(block_num);

                if let Some((persist_block, persist_cursor)) = cursor_to_persist {
                    tracing::debug!(block = persist_block, "Block fully cached, persisting cursor");
                    if let Err(e) = cache
                        .lock()
                        .await
                        .persist_cursor(INDEXER_ID, &persist_cursor, persist_block)
                        .await
                    {
                        tracing::error!(error = %e, "Failed to persist cursor");
                    }
                }

                drop(permit);
            });
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
        Ok(cursor)
    }
}

/// Process a single edit event by fetching its IPFS content.
async fn process_edit_event(
    edit: EditsPublished,
    cache: &Arc<Mutex<Cache>>,
    ipfs: &Arc<IpfsClient>,
    block_timestamp: &str,
    block_number: u64,
) -> Result<(), CacheError> {
    // Extract the IPFS URI from the edit data
    // The data field contains the IPFS CID as a UTF-8 string
    let uri = String::from_utf8_lossy(&edit.data).to_string();

    // Convert space_id bytes to hex string
    let space_id = hex::encode(&edit.space_id);

    tracing::debug!(
        uri = %uri,
        space_id = %space_id,
        block = block_number,
        "Fetching IPFS content"
    );

    // Fetch and decode the IPFS content
    let result = ipfs.get(&uri).await;

    let item = match result {
        Ok(decoded_edit) => {
            tracing::info!(
                uri = %uri,
                block = block_number,
                "Successfully cached IPFS content"
            );
            CacheItem {
                uri,
                json: Some(decoded_edit),
                block: block_timestamp.to_string(),
                space_id,
                is_errored: false,
            }
        }
        Err(error) => {
            tracing::warn!(
                uri = %uri,
                block = block_number,
                error = %error,
                "Failed to fetch/decode IPFS content"
            );
            // Still cache an errored entry so consumers know the event exists
            // but the content is invalid
            CacheItem {
                uri,
                json: None,
                block: block_timestamp.to_string(),
                space_id,
                is_errored: true,
            }
        }
    };

    // Store in cache (upsert - skips if URI already exists)
    let cache_guard = cache.lock().await;
    cache_guard.put(&item).await?;

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
        assert_eq!(result, Some((100, "cursor_100".to_string())));

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
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string())));

        assert!(pending.blocks.is_empty());
    }

    #[test]
    fn pending_fetches_multiple_blocks_complete_in_order() {
        let mut pending = PendingFetches::default();

        pending.add_block(100, "cursor_100".to_string(), 2);
        pending.add_block(101, "cursor_101".to_string(), 1);

        // Complete block 100 first
        assert_eq!(pending.complete_one(100), None); // 1 remaining
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string())));

        // Now complete block 101
        assert_eq!(pending.complete_one(101), Some((101, "cursor_101".to_string())));

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
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string())));

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
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string())));

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
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string())));

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
        assert_eq!(pending.complete_one(100), Some((100, "cursor_100".to_string()))); // 100: 0 remaining

        assert!(pending.blocks.is_empty());
    }
}
