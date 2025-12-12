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

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::future::join_all;
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::Retry;

use hermes_relay::{actions, Action};
use hermes_schema::pb::knowledge::HermesEdit;
use wire::pb::grc20::Edit;

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
pub async fn transform<C: IpfsCache + 'static>(
    actions: &[Action],
    meta: &BlockMetadata,
    cache: &Arc<C>,
    retry_config: &RetryConfig,
) -> Result<TransformResult> {
    // Collect all edit requests
    let requests: Vec<(EditRequest, Action)> = actions
        .iter()
        .filter(|action| actions::matches(&action.action, &actions::EDITS_PUBLISHED))
        .map(|action| {
            let ipfs_hash = String::from_utf8_lossy(&action.data).to_string();
            (
                EditRequest {
                    ipfs_hash,
                    space_id: action.from_id.clone(),
                },
                action.clone(),
            )
        })
        .collect();

    if requests.is_empty() {
        return Ok(TransformResult::default());
    }

    // Fetch all edits in parallel
    let fetch_results = fetch_edits_parallel(requests, meta, cache, retry_config).await;

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
async fn fetch_edits_parallel<C: IpfsCache + 'static>(
    requests: Vec<(EditRequest, Action)>,
    meta: &BlockMetadata,
    cache: &Arc<C>,
    retry_config: &RetryConfig,
) -> Vec<EditFetchResult> {
    let handles: Vec<_> = requests
        .into_iter()
        .map(|(req, action)| {
            let cache = Arc::clone(cache);
            let retry_config = retry_config.clone();
            let meta = meta.clone();

            tokio::spawn(async move {
                fetch_edit_with_retry(req, &action, &meta, &cache, &retry_config).await
            })
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
async fn fetch_edit_with_retry<C: IpfsCache>(
    req: EditRequest,
    action: &Action,
    meta: &BlockMetadata,
    cache: &C,
    config: &RetryConfig,
) -> EditFetchResult {
    let retry_strategy = ExponentialBackoff::from_millis(config.initial_delay_ms)
        .factor(config.factor)
        .max_delay(config.max_delay)
        .map(jitter)
        .take(config.max_retries);

    let result = Retry::spawn(retry_strategy, || async {
        cache.get(&req.ipfs_hash, &req.space_id).await
    })
    .await;

    match result {
        Ok(cached_edit) => {
            if cached_edit.is_errored {
                EditFetchResult::Errored
            } else if let Some(edit) = cached_edit.edit {
                match convert(action, &edit, meta) {
                    Ok(event) => EditFetchResult::Success(Box::new(event)),
                    Err(_) => EditFetchResult::FetchFailed,
                }
            } else {
                // Entry exists but no edit content
                EditFetchResult::Errored
            }
        }
        Err(CacheError::NotFound(_)) => EditFetchResult::CacheMiss,
        Err(_) => EditFetchResult::FetchFailed,
    }
}

/// Convert an EDITS_PUBLISHED action with cached edit to HermesEdit proto.
///
/// The action structure for EDITS_PUBLISHED:
/// - from_id: space_id (16 bytes) - the space publishing the edit
/// - to_id: unused (zeros)
/// - topic: unused (zeros)
/// - data: IPFS hash as bytes
fn convert(action: &Action, edit: &Edit, meta: &BlockMetadata) -> Result<HermesEdit> {
    Ok(HermesEdit {
        id: edit.id.clone(),
        name: edit.name.clone(),
        ops: edit.ops.clone(),
        authors: edit.authors.clone(),
        language: edit.language.clone(),
        space_id: hex::encode(&action.from_id),
        is_canonical: true, // TODO: Determine from topology
        meta: Some(meta.to_proto()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::pb::grc20::{Entity, Op, Value};

    fn test_meta() -> BlockMetadata {
        BlockMetadata {
            cursor: "test_cursor".to_string(),
            block_number: 12345,
            timestamp: "1234567890".to_string(),
        }
    }

    fn test_edit() -> Edit {
        Edit {
            id: vec![1; 16],
            name: "Test Edit".into(),
            ops: vec![Op {
                payload: Some(wire::pb::grc20::op::Payload::UpdateEntity(Entity {
                    id: vec![2; 16],
                    values: vec![Value {
                        property: vec![3; 16],
                        value: "test".into(),
                        options: None,
                    }],
                })),
            }],
            authors: vec![vec![4; 32]],
            language: None,
        }
    }

    #[test]
    fn test_convert_edit() {
        let action = Action {
            from_id: vec![0x01; 16],
            to_id: vec![0; 16],
            action: actions::EDITS_PUBLISHED.to_vec(),
            topic: vec![0; 32],
            data: b"QmTestHash".to_vec(),
        };

        let edit = test_edit();
        let result = convert(&action, &edit, &test_meta()).unwrap();

        assert_eq!(result.id, vec![1; 16]);
        assert_eq!(result.name, "Test Edit");
        assert_eq!(result.ops.len(), 1);
        assert_eq!(result.space_id, hex::encode(vec![0x01; 16]));
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
