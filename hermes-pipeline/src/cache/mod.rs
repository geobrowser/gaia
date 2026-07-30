//! IPFS cache module for resolving edit content from IPFS hashes.
//!
//! The cache provides a way to look up edit content by IPFS hash.
//! For mock mode, we use an in-memory cache with pre-populated test edits.
//! For production, we use PostgreSQL backed by `hermes-ipfs-cache`.
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_pipeline::cache::CacheSource;
//!
//! // Development: use in-memory cache with mock data
//! let cache = CacheSource::mock().into_cache().await?;
//!
//! // Production: use PostgreSQL
//! let cache = CacheSource::live("postgres://...").into_cache().await?;
//! ```

mod mock;
mod postgres;

pub use mock::MockIpfsCache;
pub use postgres::PostgresCache;

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

/// Errors that can occur when reading from the IPFS cache.
#[derive(Error, Debug)]
pub enum CacheError {
    /// The requested IPFS hash was not found in the cache.
    /// This can happen if the cache service hasn't processed it yet.
    #[error("Cache entry not found: {0}")]
    NotFound(String),

    /// A database error occurred (for PostgreSQL backend).
    #[error("Database error: {0}")]
    #[allow(dead_code)] // Used by PostgreSQL backend
    Database(String),
}

/// A cached edit entry with metadata.
///
/// This struct captures the various states a cache entry can be in:
/// - Successfully cached with validated GRC-20 v2 payload bytes
/// - Errored (IPFS fetch failed, invalid CID, validation failed, etc.)
/// - Missing payload (entry exists but no data)
///
/// Note: As of v2, the payload contains raw GRC2/GRC2Z bytes that have been
/// validated by hermes-ipfs-cache. Consumers (e.g., kg-indexer) must decode
/// using the grc-20 crate.
#[derive(Clone, Debug)]
pub struct CachedEdit {
    /// The IPFS CID/hash that was looked up.
    #[allow(dead_code)] // Part of public API for future use
    pub cid: String,

    /// Raw GRC2/GRC2Z payload bytes, if available.
    /// These have been validated by hermes-ipfs-cache.
    /// Will be `None` if the entry is errored or has no data.
    ///
    /// **Always check `is_errored` first, or use `valid_payload()`** —
    /// reading payload from an errored entry is meaningless.
    pub payload: Option<Vec<u8>>,

    /// Whether the cache entry is marked as errored.
    /// This happens when:
    /// - IPFS fetch failed (invalid CID, gateway timeout, not available)
    /// - GRC-20 v2 validation failed (invalid format)
    pub is_errored: bool,

    /// The space ID that published this edit (from the action).
    #[allow(dead_code)] // Part of public API for future use
    pub space_id: Vec<u8>,

    /// The edit name extracted from the GRC-20 payload.
    /// Used for populating human-readable proposal names.
    pub name: Option<String>,
}

impl CachedEdit {
    /// Create a successful cache entry with validated payload bytes.
    pub fn success(cid: String, payload: Vec<u8>, space_id: Vec<u8>, name: Option<String>) -> Self {
        Self {
            cid,
            payload: Some(payload),
            is_errored: false,
            space_id,
            name,
        }
    }

    /// Create an errored cache entry.
    pub fn errored(cid: String, space_id: Vec<u8>) -> Self {
        Self {
            cid,
            payload: None,
            is_errored: true,
            space_id,
            name: None,
        }
    }

    /// Check if this entry has valid payload content.
    #[allow(dead_code)] // Used in tests and future use
    pub fn has_content(&self) -> bool {
        !self.is_errored && self.payload.is_some()
    }

    /// Returns the payload bytes only if the entry is valid.
    /// Prefer this over reading `payload` directly — it can't forget the
    /// `is_errored` check.
    pub fn valid_payload(&self) -> Option<&[u8]> {
        if self.is_errored {
            return None;
        }
        self.payload.as_deref()
    }
}

/// Trait for IPFS cache implementations.
///
/// The cache maps IPFS hashes to Edit protos. Different implementations
/// can be used for testing (mock) vs production (PostgreSQL).
///
/// All methods are async to support database backends that require
/// asynchronous I/O.
#[async_trait]
pub trait IpfsCache: Send + Sync {
    /// Look up a single edit by its IPFS hash.
    ///
    /// # Arguments
    /// * `ipfs_hash` - The IPFS CID to look up
    /// * `space_id` - The space ID from the action (for context)
    ///
    /// # Returns
    /// * `Ok(CachedEdit)` - The cached entry (may be errored)
    /// * `Err(CacheError::NotFound)` - Entry doesn't exist in cache
    /// * `Err(CacheError::Database)` - Database connection error
    /// * `Err(CacheError::DeserializeError)` - Failed to parse cached data
    async fn get(&self, ipfs_hash: &str, space_id: &[u8]) -> Result<CachedEdit, CacheError>;

    /// Block height that `hermes-ipfs-cache` (the warmer) has durably processed.
    ///
    /// This is what lets a caller tell "the warmer has not fetched this URI
    /// YET" apart from "the warmer looked and the content is unfetchable".
    /// The warmer writes a row for every URI it sees — content on success,
    /// `is_errored = true` on failure — so a MISSING row is only meaningful
    /// once the warmer has advanced past the block that referenced it.
    ///
    /// Returns `None` when the progress is unknown, which callers must treat
    /// as "not yet processed" (the safe direction: wait rather than drop).
    ///
    /// Required rather than defaulted on purpose: a cache that silently
    /// inherited `None` would make the pipeline wait out its full backstop on
    /// every miss, so each implementation must say what it knows.
    async fn warmer_block(&self) -> Option<u64>;

    /// Batch lookup multiple edits by their IPFS hashes.
    ///
    /// This is more efficient than multiple individual lookups for backends
    /// that support batch queries. The default implementation just calls
    /// `get` for each hash sequentially.
    ///
    /// # Arguments
    /// * `requests` - List of (ipfs_hash, space_id) tuples to look up
    ///
    /// # Returns
    /// A vector of results, one for each request. The order matches the input.
    #[allow(dead_code)] // Part of public API for future use
    async fn get_batch(&self, requests: &[(&str, &[u8])]) -> Vec<Result<CachedEdit, CacheError>> {
        let mut results = Vec::with_capacity(requests.len());
        for (ipfs_hash, space_id) in requests {
            results.push(self.get(ipfs_hash, space_id).await);
        }
        results
    }
}

/// Blanket implementation for Arc<T> where T: IpfsCache
/// This allows passing Arc<Cache> to functions expecting &impl IpfsCache
#[async_trait]
impl<T: IpfsCache + ?Sized> IpfsCache for Arc<T> {
    // Must forward. When this method carried a default body, this blanket impl
    // silently inherited it, so every `Arc<dyn IpfsCache>` reported `None`
    // regardless of the concrete cache underneath — the reason `warmer_block`
    // is a required method now.
    async fn warmer_block(&self) -> Option<u64> {
        (**self).warmer_block().await
    }

    async fn get(&self, ipfs_hash: &str, space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        (**self).get(ipfs_hash, space_id).await
    }

    async fn get_batch(&self, requests: &[(&str, &[u8])]) -> Vec<Result<CachedEdit, CacheError>> {
        (**self).get_batch(requests).await
    }
}

// =============================================================================
// CacheSource - Configuration for cache backends
// =============================================================================

/// Configuration for the IPFS cache backend.
///
/// Use this to choose between mock (in-memory) and live (PostgreSQL) storage,
/// following the same pattern as `StreamSource`.
#[derive(Debug, Clone)]
pub enum CacheSource {
    /// Use in-memory cache with mock data for testing/development.
    Mock,

    /// Use PostgreSQL storage backed by `hermes-ipfs-cache`.
    Live {
        /// PostgreSQL connection URL
        database_url: String,
    },
}

impl CacheSource {
    /// Create a mock (in-memory) cache source.
    pub fn mock() -> Self {
        Self::Mock
    }

    /// Create a live cache source with the given PostgreSQL URL.
    pub fn live(database_url: impl Into<String>) -> Self {
        Self::Live {
            database_url: database_url.into(),
        }
    }

    /// Create the cache with the appropriate storage backend.
    pub async fn into_cache(self) -> Result<Arc<dyn IpfsCache>, CacheError> {
        match self {
            Self::Mock => Ok(Arc::new(MockIpfsCache::new())),
            Self::Live { database_url } => {
                let cache = PostgresCache::new(&database_url).await?;
                Ok(Arc::new(cache))
            }
        }
    }
}
