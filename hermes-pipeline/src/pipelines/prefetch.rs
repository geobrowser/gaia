//! Prefetch module for batch IPFS cache lookups.
//!
//! This module scans all actions in a block to collect IPFS URIs that need to be
//! fetched from the cache, then performs a single batch lookup. The results are
//! stored in a map that can be passed to transform functions.
//!
//! This approach:
//! - Minimizes database round-trips (single batch query per block)
//! - Keeps transform functions synchronous (no async I/O during transform)
//! - Centralizes retry logic in one place

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hermes_instrumentation::{Instrument, debug_span, info_span, warn};
use tokio_retry::Retry;
use tokio_retry::strategy::{ExponentialBackoff, jitter};

use hermes_relay::{Action, actions, extract_ipfs_uri};

use crate::cache::{CacheError, CachedEdit, IpfsCache};
use crate::decode::{
    ProposalActionType, decode_edits_published_args, decode_ping_args, decode_proposal_created,
    decode_publish_args,
};

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
    /// How long to hold a block waiting for the IPFS warmer to reach it before
    /// giving up and treating the misses as unfetchable (default: 10 minutes,
    /// override with `IPFS_WARMER_WAIT_SECS`).
    ///
    /// A backstop only. In the normal case the wait ends as soon as the warmer's
    /// cursor passes this block, which is typically seconds. Set to zero to
    /// restore the old drop-immediately behaviour.
    pub warmer_wait_max: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 10,
            factor: 2,
            max_delay: Duration::from_secs(5),
            max_retries: 10,
            warmer_wait_max: std::env::var("IPFS_WARMER_WAIT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(600)),
        }
    }
}

/// A request to fetch an IPFS URI from the cache.
#[derive(Debug, Clone)]
struct FetchRequest {
    /// The IPFS URI (e.g., "ipfs://Qm...")
    uri: String,
    /// The space ID for context
    space_id: Vec<u8>,
}

/// Result of prefetching all IPFS URIs for a block.
#[derive(Debug, Default)]
pub struct PrefetchResult {
    /// Map of IPFS URI to cached edit. Includes both successful lookups and
    /// errored entries — consumers must check `cached_edit.is_errored` (or
    /// use `valid_payload()`) before reading payload. Cache misses (NotFound
    /// after retries) are not inserted.
    pub cache: HashMap<String, CachedEdit>,
    /// Number of cache misses (after all retries exhausted).
    pub cache_misses: u64,
    /// Number of errored entries (cache marked them as failed).
    pub errored_entries: u64,
    /// Number of fetch failures (database errors, etc.).
    pub fetch_failures: u64,
}

/// Collect all IPFS URIs that need to be fetched for a block.
///
/// Scans:
/// - EDITS_PUBLISHED actions → content URI
/// - PROPOSAL_CREATED/UPDATED actions → Publish action content URIs
fn collect_uris(actions: &[Action]) -> Vec<FetchRequest> {
    let mut requests = Vec::new();

    for action in actions {
        let action_type = action.action.as_slice();

        // EDITS_PUBLISHED: extract IPFS URI from action data
        if actions::matches(action_type, &actions::EDITS_PUBLISHED)
            && let Some(ipfs_uri) = extract_ipfs_uri(&action.data)
        {
            requests.push(FetchRequest {
                uri: ipfs_uri,
                space_id: action.from_id.clone(),
            });
        }

        // PROPOSAL_CREATED/UPDATED: extract Publish action content URIs
        if (actions::matches(action_type, &actions::PROPOSAL_CREATED)
            || actions::matches(action_type, &actions::PROPOSAL_UPDATED))
            && let Ok((decoded, _)) = decode_proposal_created(&action.data)
        {
            for proposal_action in decoded.actions {
                let action_type = ProposalActionType::from_calldata(&proposal_action.data);
                if matches!(action_type, ProposalActionType::Publish)
                    && let Ok(args) = decode_publish_args(&proposal_action.data)
                {
                    requests.push(FetchRequest {
                        uri: args.content_uri,
                        space_id: action.to_id.clone(), // space owning the proposal
                    });
                }

                // Edit proposals arrive as ping(EDITS_PUBLISHED, ...); extract
                // the content URI so name enrichment has cached edit data.
                if matches!(action_type, ProposalActionType::Ping)
                    && let Ok(ping) = decode_ping_args(&proposal_action.data)
                    && ping.action == actions::EDITS_PUBLISHED
                    && let Ok(args) = decode_edits_published_args(&ping.data)
                {
                    requests.push(FetchRequest {
                        uri: args.content_uri,
                        space_id: action.to_id.clone(), // space owning the proposal
                    });
                }
            }
        }
    }

    // Deduplicate by URI (same URI might appear multiple times)
    let mut seen = std::collections::HashSet::new();
    requests.retain(|req| seen.insert(req.uri.clone()));

    requests
}

/// Prefetch all IPFS URIs needed for a block.
///
/// This performs a batch lookup with retry logic for cache misses.
/// How long to wait between re-checks while the warmer catches up.
const WARMER_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Emit an escalating log every this many waits, so a genuinely stuck warmer is
/// loud rather than a silent stall.
const WARMER_STALL_LOG_EVERY: u32 = 15;

pub async fn prefetch_block(
    actions: &[Action],
    cache: &Arc<dyn IpfsCache>,
    config: &RetryConfig,
    block_number: u64,
) -> PrefetchResult {
    let requests = collect_uris(actions);

    if requests.is_empty() {
        return PrefetchResult::default();
    }

    let request_count = requests.len();
    info_span!("prefetch.batch", count = request_count).in_scope(|| {
        tracing::info!("Prefetching {} IPFS URIs", request_count);
    });

    // A cache miss has two very different causes, and conflating them loses
    // data: either the warmer has not fetched this URI YET (transient — the
    // pipeline is simply ahead of it at chain head), or the warmer looked and
    // the content is unfetchable (permanent, recorded as `is_errored`).
    //
    // The warmer writes a row for EVERY URI it processes, so a missing row only
    // means "unfetchable" once the warmer has advanced past this block. Until
    // then we wait instead of dropping. Waiting is bounded by the warmer's own
    // progress, not by a timeout, so this cannot stall on content that the
    // warmer has already ruled on.
    let mut waits: u32 = 0;
    loop {
        let result = fetch_once(&requests, cache, config).await;

        if result.cache_misses == 0 {
            return result;
        }

        match cache.warmer_block().await {
            // Warmer is past this block: the misses are real verdicts.
            Some(warmer) if warmer >= block_number => {
                tracing::error!(
                    block_number,
                    warmer_block = warmer,
                    count = result.cache_misses,
                    "Cache misses persist though the IPFS warmer is past this block; \
                     treating as unfetchable"
                );
                return result;
            }
            // Warmer is behind (or unknown): these URIs are simply not fetched
            // yet. Hold the block — do NOT let the cursor advance past it.
            other => {
                let waited = WARMER_POLL_INTERVAL * waits;
                if waited >= config.warmer_wait_max {
                    tracing::error!(
                        block_number,
                        warmer_block = ?other,
                        count = result.cache_misses,
                        waited_secs = waited.as_secs(),
                        "Gave up waiting for the IPFS warmer; edits in this block will be \
                         dropped. The warmer is stalled or not running — investigate before \
                         trusting this block's contents"
                    );
                    return result;
                }
                waits += 1;
                if waits % WARMER_STALL_LOG_EVERY == 1 {
                    tracing::warn!(
                        block_number,
                        warmer_block = ?other,
                        outstanding = result.cache_misses,
                        waited_secs = waits * WARMER_POLL_INTERVAL.as_secs() as u32,
                        "Waiting for the IPFS warmer to reach this block before emitting; \
                         edits would otherwise be dropped"
                    );
                }
                tokio::time::sleep(WARMER_POLL_INTERVAL).await;
            }
        }
    }
}

/// One parallel pass over every URI in the block.
async fn fetch_once(
    requests: &[FetchRequest],
    cache: &Arc<dyn IpfsCache>,
    config: &RetryConfig,
) -> PrefetchResult {
    // Fetch all in parallel with retry
    let fetch_futures = requests.iter().map(|request| {
        let cache = Arc::clone(cache);
        let config = config.clone();
        let uri = request.uri.clone();
        let space_id = request.space_id.clone();

        async move {
            let space_id_hex = hex::encode(&space_id);
            let request = FetchRequest {
                uri: uri.clone(),
                space_id: space_id.clone(),
            };
            let fetch_result = debug_span!("prefetch.fetch", uri = %uri, space_id = %space_id_hex)
                .in_scope(|| fetch_with_retry(&request, &cache, &config))
                .await;
            (uri, fetch_result)
        }
    });

    let fetch_results = futures::future::join_all(fetch_futures).await;

    // Collect results
    let mut result = PrefetchResult::default();
    for (uri, fetch_result) in fetch_results {
        match fetch_result {
            FetchResult::Success(cached_edit) => {
                tracing::debug!(uri = %uri, "Prefetch success");
                result.cache.insert(uri, cached_edit);
            }
            FetchResult::CacheMiss => {
                tracing::warn!(uri = %uri, "Prefetch cache miss after retries");
                result.cache_misses += 1;
            }
            FetchResult::Errored(cached_edit) => {
                tracing::warn!(uri = %uri, "Prefetch found errored entry in cache");
                result.cache.insert(uri, cached_edit);
                result.errored_entries += 1;
            }
            FetchResult::FetchFailed => {
                tracing::error!(uri = %uri, "Prefetch database error");
                result.fetch_failures += 1;
            }
        }
    }

    result
}

/// Result of a single fetch operation.
enum FetchResult {
    Success(CachedEdit),
    CacheMiss,
    Errored(CachedEdit),
    FetchFailed,
}

/// Fetch a single URI with retry logic.
async fn fetch_with_retry(
    request: &FetchRequest,
    cache: &Arc<dyn IpfsCache>,
    config: &RetryConfig,
) -> FetchResult {
    let retry_strategy = ExponentialBackoff::from_millis(config.initial_delay_ms)
        .factor(config.factor)
        .max_delay(config.max_delay)
        .map(jitter)
        .take(config.max_retries);

    let uri = request.uri.clone();
    let space_id = request.space_id.clone();
    let space_id_hex = hex::encode(&space_id);
    let attempts = Arc::new(AtomicUsize::new(0));

    let result = Retry::spawn(retry_strategy, || {
        let uri = uri.clone();
        let space_id = space_id.clone();
        let space_id_hex = space_id_hex.clone();
        let cache = Arc::clone(cache);
        let attempts = Arc::clone(&attempts);

        let span = debug_span!("cache.get", uri = %uri, space_id = %space_id_hex);
        async move {
            let result = cache.get(&uri, &space_id).await;
            let attempt = attempts.fetch_add(1, Ordering::Relaxed) + 1;

            if matches!(result, Err(CacheError::NotFound(_))) {
                warn!(
                    uri = %uri,
                    space_id = %space_id_hex,
                    attempt,
                    "Cache miss, retrying"
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
                FetchResult::Errored(cached_edit)
            } else if cached_edit.payload.is_some() {
                FetchResult::Success(cached_edit)
            } else {
                // Entry exists but no payload — normalize to an errored entry
                // so the invariant `is_errored == true ⇔ in_cache_as_errored`
                // holds for downstream consumers.
                FetchResult::Errored(CachedEdit::errored(cached_edit.cid, cached_edit.space_id))
            }
        }
        Err(CacheError::NotFound(_)) => {
            let attempts = attempts.load(Ordering::Relaxed);
            warn!(
                uri = %request.uri,
                attempts,
                max_retries = config.max_retries,
                "Cache miss after retries exhausted"
            );
            FetchResult::CacheMiss
        }
        Err(_) => FetchResult::FetchFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::MockIpfsCache;

    #[test]
    fn test_collect_uris_deduplicates() {
        // Same URI appearing in multiple places should only be fetched once
        let requests = vec![
            FetchRequest {
                uri: "ipfs://Qm123".to_string(),
                space_id: vec![1; 16],
            },
            FetchRequest {
                uri: "ipfs://Qm123".to_string(),
                space_id: vec![2; 16],
            },
            FetchRequest {
                uri: "ipfs://Qm456".to_string(),
                space_id: vec![1; 16],
            },
        ];

        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<_> = requests
            .into_iter()
            .filter(|req| seen.insert(req.uri.clone()))
            .collect();

        assert_eq!(deduped.len(), 2);
    }

    /// Fast retry config so cache-miss tests don't sit on the default 10-retry
    /// exponential backoff.
    fn fast_retry_config() -> RetryConfig {
        RetryConfig {
            initial_delay_ms: 1,
            factor: 1,
            max_delay: Duration::from_millis(1),
            max_retries: 1,
            // Tests assert the drop path directly; no warmer to wait for.
            warmer_wait_max: Duration::ZERO,
        }
    }

    /// Cache whose warmer position is fixed, and whose content for a given hash
    /// only materialises after N lookups — models the pipeline running ahead of
    /// the warmer at chain head.
    struct LaggingCache {
        inner: MockIpfsCache,
        warmer: Option<u64>,
        appears_after: usize,
        looks: std::sync::atomic::AtomicUsize,
        hash: String,
    }

    #[async_trait::async_trait]
    impl IpfsCache for LaggingCache {
        async fn warmer_block(&self) -> Option<u64> {
            self.warmer
        }

        async fn get(&self, ipfs_hash: &str, space_id: &[u8]) -> Result<CachedEdit, CacheError> {
            if ipfs_hash == self.hash {
                let n = self.looks.fetch_add(1, Ordering::Relaxed) + 1;
                if n < self.appears_after {
                    return Err(CacheError::NotFound(ipfs_hash.to_string()));
                }
                // Once the warmer has fetched it, serve a real cached edit.
                return self.inner.get("QmRootEdit1CreatePersons", space_id).await;
            }
            self.inner.get(ipfs_hash, space_id).await
        }
    }

    // The regression this guards: the pipeline reaching a block before the
    // warmer used to drop the edit permanently and advance its cursor, silently
    // losing user writes. It must wait instead.
    #[tokio::test]
    async fn does_not_drop_when_warmer_is_behind_and_content_arrives() {
        let cache: Arc<dyn IpfsCache> = Arc::new(LaggingCache {
            inner: MockIpfsCache::new(),
            warmer: Some(50), // behind the block below
            appears_after: 3,
            looks: std::sync::atomic::AtomicUsize::new(0),
            hash: format!("ipfs://{}", MISSING_HASH),
        });
        let actions = [edits_published_action(MISSING_HASH, 0x01)];

        let mut config = fast_retry_config();
        config.warmer_wait_max = Duration::from_secs(30);

        let result = prefetch_block(&actions, &cache, &config, 100).await;

        assert_eq!(
            result.cache_misses, 0,
            "must not drop while warmer is behind"
        );
        assert_eq!(result.cache.len(), 1, "edit must be recovered once fetched");
    }

    // The other half: once the warmer is PAST the block, a missing row is a real
    // verdict and must not hang the pipeline.
    #[tokio::test]
    async fn drops_when_warmer_has_already_passed_the_block() {
        let cache: Arc<dyn IpfsCache> = Arc::new(LaggingCache {
            inner: MockIpfsCache::new(),
            warmer: Some(100), // at/past the block below
            appears_after: usize::MAX,
            looks: std::sync::atomic::AtomicUsize::new(0),
            hash: format!("ipfs://{}", MISSING_HASH),
        });
        let actions = [edits_published_action(MISSING_HASH, 0x01)];

        let mut config = fast_retry_config();
        config.warmer_wait_max = Duration::from_secs(30);

        let result = prefetch_block(&actions, &cache, &config, 100).await;

        assert_eq!(result.cache_misses, 1);
        assert!(result.cache.is_empty());
    }

    fn edits_published_action(ipfs_uri: &str, space_id: u8) -> Action {
        Action {
            from_id: vec![space_id; 16],
            to_id: vec![0; 16],
            action: actions::EDITS_PUBLISHED.to_vec(),
            topic: vec![0; 32],
            data: format!("ipfs://{}", ipfs_uri).into_bytes(),
        }
    }

    /// A valid 46-char CIDv0 hash that's not in the MockIpfsCache.
    /// Reused across the two prefetch tests below for symmetry.
    const ERRORED_HASH: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
    const MISSING_HASH: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdH";

    #[tokio::test]
    async fn errored_entries_are_inserted_into_cache_map() {
        // prefetch_block calls cache.get(uri, ...) with the full "ipfs://..."
        // URI as the lookup key, so the errored set must use the full URI too.
        let key = format!("ipfs://{}", ERRORED_HASH);
        let cache: Arc<dyn IpfsCache> =
            Arc::new(MockIpfsCache::with_errored_hashes(vec![key.clone()]));
        let actions = [edits_published_action(ERRORED_HASH, 0x01)];

        let result = prefetch_block(&actions, &cache, &fast_retry_config(), 1).await;

        let entry = result.cache.get(&key).expect(
            "errored entries must be inserted into the prefetched map so edits.rs can \
             distinguish them from cache misses",
        );
        assert!(
            entry.is_errored,
            "inserted entry must carry is_errored=true"
        );
        assert_eq!(result.errored_entries, 1);
        assert_eq!(result.cache_misses, 0);
        assert_eq!(result.fetch_failures, 0);
    }

    #[tokio::test]
    async fn cache_misses_are_not_inserted_into_cache_map() {
        // MockIpfsCache returns NotFound for hashes it doesn't know about, and
        // we don't mark this hash as errored. So `fetch_with_retry` exhausts
        // its 1 retry and lands on FetchResult::CacheMiss.
        let cache: Arc<dyn IpfsCache> = Arc::new(MockIpfsCache::new());
        let actions = [edits_published_action(MISSING_HASH, 0x01)];

        let result = prefetch_block(&actions, &cache, &fast_retry_config(), 1).await;

        let key = format!("ipfs://{}", MISSING_HASH);
        assert!(
            !result.cache.contains_key(&key),
            "cache misses must NOT be inserted into the prefetched map; \
             edits.rs uses the `None` branch as the live indicator of a genuine miss"
        );
        assert_eq!(result.cache_misses, 1);
        assert_eq!(result.errored_entries, 0);
        assert_eq!(result.fetch_failures, 0);
    }
}
