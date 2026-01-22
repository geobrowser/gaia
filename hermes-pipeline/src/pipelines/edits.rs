//! Pipeline: EDITS_PUBLISHED → knowledge.edits
//!
//! Converts edit published actions to HermesEdit events.
//! Unlike other pipelines, this requires an external cache lookup
//! to resolve IPFS hash → Edit content.
//!
//! Features:
//! - Parallel cache fetching for all edits in a block
//! - Retry logic with exponential backoff for cache misses
//! - Graceful handling of errored cache entries
//!
//! Note: As of v2, the cache returns raw GRC2/GRC2Z payload bytes.
//! We decode the header to populate HermesEdit fields for observability,
//! but the full payload is passed to kg-indexer for decoding.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use futures::future::join_all;
use grc_20::decode_edit;
use hermes_instrumentation::{Instrument, debug_span, info_span};
use tokio_retry::Retry;
use tokio_retry::strategy::{ExponentialBackoff, jitter};
use tracing::warn;

use hermes_codec::{actions, extract_ipfs_uri};
use hermes_relay::Action;
use hermes_schema::pb::knowledge::HermesEdit;

use crate::cache::{CacheError, IpfsCache};

use super::BlockMetadata;

/// Configuration for cache retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Initial delay between retries (default: 10ms)
    pub initial_delay_ms: u64,
    /// Multiplier for each subsequent retry (default: 2)
    pub factor: u64,
    /// Maximum delay between retries (default: 5s)
    pub max_delay: Duration,
    /// Maximum number of retries (default: 10)
    pub max_retries: usize,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 10,
            factor: 2,
            max_delay: Duration::from_secs(5),
            max_retries: 10,
        }
    }
}

/// Result of transforming edit actions.
#[derive(Debug, Default)]
pub struct TransformResult {
    /// Transformed edit events ready for emission.
    pub events: Vec<HermesEdit>,
    /// Number of cache misses (after all retries exhausted).
    pub cache_misses: u64,
    /// Number of errored entries (cache marked them as failed).
    pub errored_entries: u64,
    /// Number of fetch failures (database errors, etc.).
    pub fetch_failures: u64,
}

/// Request for fetching an edit from the cache.
struct EditRequest {
    ipfs_hash: String,
    space_id: Vec<u8>,
    sequence: u32,
}

/// Result of fetching an edit from the cache.
enum EditFetchResult {
    /// Successfully fetched and converted to HermesEdit.
    Success(Box<HermesEdit>),
    /// Cache miss after all retries.
    CacheMiss,
    /// Entry exists but is marked as errored.
    Errored,
    /// Fetch failed due to database/network error.
    FetchFailed,
}

/// Transform all EDITS_PUBLISHED actions in a block.
///
/// This function:
/// 1. Filters actions for EDITS_PUBLISHED
/// 2. Fetches all edits from cache in parallel with retries
/// 3. Converts successful fetches to HermesEdit events
/// 4. Returns events without sending to Kafka
pub async fn transform(
    actions: &[Action],
    meta: &BlockMetadata,
    cache: &Arc<dyn IpfsCache>,
    retry_config: &RetryConfig,
) -> Result<TransformResult> {
    // Collect all edit requests, filtering out actions without valid IPFS URIs
    let requests: Vec<(EditRequest, Action)> = actions
        .iter()
        .enumerate()
        .filter(|(_, action)| actions::matches(&action.action, &actions::EDITS_PUBLISHED))
        .filter_map(|(index, action)| {
            // Extract and validate IPFS URI from ABI-encoded action data
            match extract_ipfs_uri(&action.data) {
                Some(ipfs_hash) => Some((
                    EditRequest {
                        ipfs_hash,
                        space_id: action.from_id.clone(),
                        sequence: index as u32,
                    },
                    action.clone(),
                )),
                None => {
                    let space_id = hex::encode(&action.from_id);
                    let data_prefix_len = action.data.len().min(64);
                    let data_prefix = hex::encode(&action.data[..data_prefix_len]);
                    warn!(
                        block = meta.block_number,
                        space_id = %space_id,
                        data_len = action.data.len(),
                        data_prefix = %data_prefix,
                        "EDITS_PUBLISHED missing valid IPFS URI, skipping"
                    );
                    None
                }
            }
        })
        .collect();

    if requests.is_empty() {
        return Ok(TransformResult::default());
    }

    // Fetch all edits in parallel
    let request_count = requests.len();
    let fetch_results = fetch_edits_parallel(requests, meta, cache, retry_config)
        .instrument(info_span!("fetch.batch", count = request_count))
        .await;

    // Collect results
    let mut result = TransformResult::default();

    for fetch_result in fetch_results {
        match fetch_result {
            EditFetchResult::Success(event) => {
                result.events.push(*event);
            }
            EditFetchResult::CacheMiss => {
                result.cache_misses += 1;
            }
            EditFetchResult::Errored => {
                result.errored_entries += 1;
            }
            EditFetchResult::FetchFailed => {
                result.fetch_failures += 1;
            }
        }
    }

    Ok(result)
}

/// Fetch all edits in parallel with retry logic.
async fn fetch_edits_parallel(
    requests: Vec<(EditRequest, Action)>,
    meta: &BlockMetadata,
    cache: &Arc<dyn IpfsCache>,
    retry_config: &RetryConfig,
) -> Vec<EditFetchResult> {
    let handles: Vec<_> = requests
        .into_iter()
        .map(|(req, action)| {
            let cache = Arc::clone(cache);
            let retry_config = retry_config.clone();
            let meta = meta.clone();
            let ipfs_hash = req.ipfs_hash.clone();

            tokio::spawn(
                async move {
                    fetch_edit_with_retry(req, &action, &meta, cache, &retry_config).await
                }
                .instrument(info_span!("fetch.edit", ipfs_hash = %ipfs_hash)),
            )
        })
        .collect();

    // Wait for all fetches to complete
    let results = join_all(handles).await;

    // Unwrap the JoinHandle results
    results
        .into_iter()
        .map(|r| r.unwrap_or(EditFetchResult::FetchFailed))
        .collect()
}

/// Fetch a single edit with retry logic and convert to HermesEdit.
async fn fetch_edit_with_retry(
    req: EditRequest,
    action: &Action,
    meta: &BlockMetadata,
    cache: Arc<dyn IpfsCache>,
    config: &RetryConfig,
) -> EditFetchResult {
    let retry_strategy = ExponentialBackoff::from_millis(config.initial_delay_ms)
        .factor(config.factor)
        .max_delay(config.max_delay)
        .map(jitter)
        .take(config.max_retries);

    let ipfs_hash = req.ipfs_hash.clone();
    let space_id = req.space_id.clone();
    let sequence = req.sequence;
    let attempts = Arc::new(AtomicUsize::new(0));
    let result = Retry::spawn(retry_strategy, || {
        let hash = ipfs_hash.clone();
        let space = space_id.clone();
        let cache = Arc::clone(&cache);
        let attempts = Arc::clone(&attempts);
        let span = debug_span!("cache.get", ipfs_hash = %hash);
        async move {
            let result = cache.get(&hash, &space).await;
            let attempt = attempts.fetch_add(1, Ordering::Relaxed) + 1;
            if matches!(result, Err(CacheError::NotFound(_))) {
                warn!(
                    ipfs_hash = %hash,
                    tags.ipfs_hash = %hash,
                    attempt,
                    "Edit cache miss, retrying"
                );
            }
            result
        }
        .instrument(span)
    })
    .await;

    match result {
        Ok(cached_edit) => {
            if cached_edit.is_errored {
                EditFetchResult::Errored
            } else if let Some(payload) = cached_edit.payload {
                debug_span!("convert").in_scope(|| {
                    match convert(action, &payload, meta, sequence) {
                        Ok(event) => EditFetchResult::Success(Box::new(event)),
                        Err(_) => EditFetchResult::FetchFailed,
                    }
                })
            } else {
                // Entry exists but no payload content
                EditFetchResult::Errored
            }
        }
        Err(CacheError::NotFound(_)) => {
            let attempts = attempts.load(Ordering::Relaxed);
            warn!(
                ipfs_hash = %ipfs_hash,
                tags.ipfs_hash = %ipfs_hash,
                attempts,
                max_retries = config.max_retries,
                "Edit cache miss after retries exhausted"
            );
            EditFetchResult::CacheMiss
        }
        Err(_) => EditFetchResult::FetchFailed,
    }
}

/// Convert an EDITS_PUBLISHED action with cached payload to HermesEdit proto.
///
/// The action structure for EDITS_PUBLISHED:
/// - from_id: space_id (16 bytes) - the space publishing the edit
/// - to_id: unused (zeros)
/// - topic: unused (zeros)
/// - data: IPFS hash as bytes
///
/// Note: We decode the GRC2/GRC2Z payload to extract header fields (id, name,
/// authors) for observability, but the full payload bytes are passed through
/// to kg-indexer for decoding.
fn convert(
    action: &Action,
    payload: &[u8],
    meta: &BlockMetadata,
    sequence: u32,
) -> Result<HermesEdit> {
    // Decode the edit to extract header fields
    // This is validated by hermes-ipfs-cache, so decode should succeed
    let edit = decode_edit(payload)
        .map_err(|e| anyhow::anyhow!("Failed to decode GRC-20 payload: {}", e))?;

    Ok(HermesEdit {
        id: edit.id.to_vec(),
        name: edit.name.to_string(),
        payload: payload.to_vec(),
        authors: edit.authors.iter().map(|a| a.to_vec()).collect(),
        language: None, // v2 doesn't have a language field at edit level
        space_id: action.from_id.clone(),
        is_canonical: true, // TODO: Determine from topology
        meta: Some(meta.to_proto(sequence)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use grc_20::genesis::properties;
    use grc_20::{
        CreateEntity, Edit as Grc20Edit, Op, PropertyValue, Value as Grc20Value, encode_edit,
    };
    use std::borrow::Cow;

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    fn test_payload() -> Vec<u8> {
        let edit = Grc20Edit {
            id: [1u8; 16],
            name: Cow::Borrowed("Test Edit"),
            authors: vec![[4u8; 16]],
            created_at: 1700000000,
            ops: vec![Op::CreateEntity(CreateEntity {
                id: [2u8; 16],
                values: vec![PropertyValue {
                    property: properties::name(),
                    value: Grc20Value::Text {
                        value: Cow::Borrowed("test"),
                        language: None,
                    },
                }],
            })],
        };
        encode_edit(&edit).expect("Should encode test edit")
    }

    #[test]
    fn test_convert_payload() {
        let action = Action {
            from_id: vec![0x01; 16],
            to_id: vec![0; 16],
            action: actions::EDITS_PUBLISHED.to_vec(),
            topic: vec![0; 32],
            data: b"ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG".to_vec(),
        };

        let payload = test_payload();
        let result = convert(&action, &payload, &test_meta(), 0).unwrap();

        assert_eq!(result.id, vec![1u8; 16]);
        assert_eq!(result.name, "Test Edit");
        assert!(!result.payload.is_empty());
        assert_eq!(result.space_id, vec![0x01; 16]);
        assert!(result.is_canonical);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.initial_delay_ms, 10);
        assert_eq!(config.factor, 2);
        assert_eq!(config.max_delay, Duration::from_secs(5));
        assert_eq!(config.max_retries, 10);
    }
}
