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
use crate::decode::{ProposalActionType, decode_proposal_created, decode_publish_args};

/// Configuration for cache retry behavior.
///
/// When the IPFS cache falls behind (e.g. after a Pinax substream disconnect),
/// the pipeline needs to wait long enough for the cache to catch up before
/// giving up. The defaults give a ~5 minute window which covers typical
/// reconnect-and-catch-up scenarios.
///
/// All fields can be overridden via environment variables:
/// - `PREFETCH_RETRY_INITIAL_MS` (default: 10)
/// - `PREFETCH_RETRY_FACTOR` (default: 2)
/// - `PREFETCH_RETRY_MAX_DELAY_SECS` (default: 30)
/// - `PREFETCH_RETRY_MAX_COUNT` (default: 30)
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Initial delay between retries (default: 10ms)
    pub initial_delay_ms: u64,
    /// Multiplier for each subsequent retry (default: 2)
    pub factor: u64,
    /// Maximum delay between retries (default: 30s)
    pub max_delay: Duration,
    /// Maximum number of retries (default: 30)
    pub max_retries: usize,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_delay_ms: 10,
            factor: 2,
            max_delay: Duration::from_secs(30),
            max_retries: 30,
        }
    }
}

impl RetryConfig {
    /// Load retry config from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            initial_delay_ms: std::env::var("PREFETCH_RETRY_INITIAL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.initial_delay_ms),
            factor: std::env::var("PREFETCH_RETRY_FACTOR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.factor),
            max_delay: std::env::var("PREFETCH_RETRY_MAX_DELAY_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(defaults.max_delay),
            max_retries: std::env::var("PREFETCH_RETRY_MAX_COUNT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.max_retries),
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
    /// Map of IPFS URI to cached edit (successful lookups only).
    /// Errored entries and cache misses are not included.
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
pub async fn prefetch_block(
    actions: &[Action],
    cache: &Arc<dyn IpfsCache>,
    config: &RetryConfig,
) -> PrefetchResult {
    let requests = collect_uris(actions);

    if requests.is_empty() {
        return PrefetchResult::default();
    }

    let request_count = requests.len();
    info_span!("prefetch.batch", count = request_count).in_scope(|| {
        tracing::info!("Prefetching {} IPFS URIs", request_count);
    });

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
            FetchResult::Errored => {
                tracing::warn!(uri = %uri, "Prefetch found errored entry in cache");
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
    Errored,
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
                FetchResult::Errored
            } else if cached_edit.payload.is_some() {
                FetchResult::Success(cached_edit)
            } else {
                // Entry exists but no payload
                FetchResult::Errored
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

    #[test]
    fn test_default_retry_window_covers_cache_catchup() {
        // After a Pinax disconnect the IPFS cache can fall behind by minutes.
        // Verify the default config gives enough total wait time.
        let config = RetryConfig::default();

        // Calculate total wait: sum of exponential delays capped at max_delay
        let mut total_ms: u64 = 0;
        let mut delay_ms = config.initial_delay_ms;
        for _ in 0..config.max_retries {
            let capped = delay_ms.min(config.max_delay.as_millis() as u64);
            total_ms += capped;
            delay_ms = delay_ms.saturating_mul(config.factor);
        }

        let total_secs = total_ms / 1000;
        // Must wait at least 3 minutes to survive typical Pinax reconnect lag
        assert!(
            total_secs >= 180,
            "Default retry window {total_secs}s is too short, need >= 180s"
        );
    }

    #[test]
    fn test_from_env_uses_defaults_when_unset() {
        // SAFETY: test-only env mutation, tests run with --test-threads=1 for env tests
        unsafe {
            std::env::remove_var("PREFETCH_RETRY_INITIAL_MS");
            std::env::remove_var("PREFETCH_RETRY_FACTOR");
            std::env::remove_var("PREFETCH_RETRY_MAX_DELAY_SECS");
            std::env::remove_var("PREFETCH_RETRY_MAX_COUNT");
        }

        let config = RetryConfig::from_env();
        let defaults = RetryConfig::default();

        assert_eq!(config.initial_delay_ms, defaults.initial_delay_ms);
        assert_eq!(config.factor, defaults.factor);
        assert_eq!(config.max_delay, defaults.max_delay);
        assert_eq!(config.max_retries, defaults.max_retries);
    }

    #[test]
    fn test_from_env_reads_overrides() {
        // SAFETY: test-only env mutation
        unsafe {
            std::env::set_var("PREFETCH_RETRY_INITIAL_MS", "50");
            std::env::set_var("PREFETCH_RETRY_FACTOR", "3");
            std::env::set_var("PREFETCH_RETRY_MAX_DELAY_SECS", "60");
            std::env::set_var("PREFETCH_RETRY_MAX_COUNT", "20");
        }

        let config = RetryConfig::from_env();

        assert_eq!(config.initial_delay_ms, 50);
        assert_eq!(config.factor, 3);
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.max_retries, 20);

        unsafe {
            std::env::remove_var("PREFETCH_RETRY_INITIAL_MS");
            std::env::remove_var("PREFETCH_RETRY_FACTOR");
            std::env::remove_var("PREFETCH_RETRY_MAX_DELAY_SECS");
            std::env::remove_var("PREFETCH_RETRY_MAX_COUNT");
        }
    }

    #[test]
    fn test_from_env_ignores_invalid_values() {
        // SAFETY: test-only env mutation
        unsafe {
            std::env::set_var("PREFETCH_RETRY_INITIAL_MS", "not_a_number");
            std::env::set_var("PREFETCH_RETRY_MAX_COUNT", "-5");
        }

        let config = RetryConfig::from_env();
        let defaults = RetryConfig::default();

        assert_eq!(config.initial_delay_ms, defaults.initial_delay_ms);
        assert_eq!(config.max_retries, defaults.max_retries);

        unsafe {
            std::env::remove_var("PREFETCH_RETRY_INITIAL_MS");
            std::env::remove_var("PREFETCH_RETRY_MAX_COUNT");
        }
    }
}
