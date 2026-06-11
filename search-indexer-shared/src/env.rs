//! Environment-based configuration utilities for search indexer.
//!
//! Provides functions for computing environment-specific prefixes used to isolate
//! staging and production indexes.

use std::env;
use std::sync::OnceLock;

/// Cached index prefix, computed once on first access.
static INDEX_PREFIX: OnceLock<&'static str> = OnceLock::new();

/// Cached consumer group prefix, computed once on first access.
static CONSUMER_GROUP_PREFIX: OnceLock<&'static str> = OnceLock::new();

/// Get the index prefix based on the `ENVIRONMENT` variable.
///
/// Uses `OnceLock` to compute the prefix once and cache it for the lifetime
/// of the process. Returns a `&'static str` for zero-allocation usage.
///
/// - `ENVIRONMENT=staging` → returns `"staging_"`
/// - `ENVIRONMENT=testnet` → returns `"testnet_"`
/// - `ENVIRONMENT=production` → returns `""`
///
/// # Panics
///
/// Panics if `ENVIRONMENT` is not set or has an unexpected value.
///
/// # Example
///
/// ```ignore
/// use search_indexer_shared::get_index_prefix;
///
/// let prefix = get_index_prefix();
/// let alias = format!("{}entities", prefix);
/// ```
pub fn get_index_prefix() -> &'static str {
    INDEX_PREFIX.get_or_init(|| {
        let environment = env::var("ENVIRONMENT")
            .expect("ENVIRONMENT variable must be set to 'staging', 'testnet' or 'production'");
        match environment.as_str() {
            "staging" => "staging_",
            "testnet" => "testnet_",
            "production" => "",
            other => panic!(
                "ENVIRONMENT must be 'staging', 'testnet' or 'production', got '{}'",
                other
            ),
        }
    })
}

/// Get the consumer group prefix based on the `ENVIRONMENT` variable.
///
/// Uses `OnceLock` to compute the prefix once and cache it for the lifetime
/// of the process. Returns a `&'static str` for zero-allocation usage.
///
/// - `ENVIRONMENT=staging` → returns `"staging-"`
/// - `ENVIRONMENT=testnet` → returns `"testnet-"`
/// - `ENVIRONMENT=production` → returns `""`
///
/// # Panics
///
/// Panics if `ENVIRONMENT` is not set or has an unexpected value.
///
/// # Example
///
/// ```ignore
/// use search_indexer_shared::get_consumer_group_prefix;
///
/// let prefix = get_consumer_group_prefix();
/// let group_id = format!("{}search-indexer-group-edits", prefix);
/// // staging: "staging-search-indexer-group-edits"
/// // production: "search-indexer-group-edits"
/// ```
pub fn get_consumer_group_prefix() -> &'static str {
    CONSUMER_GROUP_PREFIX.get_or_init(|| {
        let environment = env::var("ENVIRONMENT")
            .expect("ENVIRONMENT variable must be set to 'staging', 'testnet' or 'production'");
        match environment.as_str() {
            "staging" => "staging-",
            "testnet" => "testnet-",
            "production" => "",
            other => panic!(
                "ENVIRONMENT must be 'staging', 'testnet' or 'production', got '{}'",
                other
            ),
        }
    })
}

#[cfg(test)]
mod tests {
    // Note: get_index_prefix() cannot be easily unit tested because it reads
    // from environment variables and uses OnceLock which persists across tests.
    // Testing would require process isolation or careful env var management.
}
