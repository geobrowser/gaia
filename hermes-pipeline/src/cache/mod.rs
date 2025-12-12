//! IPFS cache module for resolving edit content from IPFS hashes.
//!
//! The cache provides a way to look up edit content by IPFS hash.
//! For mock mode, we use an in-memory cache with pre-populated test edits.
//! For production, this would be replaced with a cache backed by the
//! `hermes-ipfs-cache` PostgreSQL database.

mod mock;

pub use mock::MockIpfsCache;

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use wire::pb::grc20::Edit;

/// Errors that can occur when reading from the IPFS cache.
#[derive(Error, Debug)]
pub enum CacheError {
    /// The requested IPFS hash was not found in the cache.
    /// This can happen if the cache service hasn't processed it yet.
    #[error("Cache entry not found: {0}")]
    NotFound(String),

    /// A database error occurred (for PostgreSQL backend).
    #[error("Database error: {0}")]
    #[allow(dead_code)] // Used by PostgreSQL backend (not yet implemented)
    Database(String),

    /// Failed to deserialize the cached JSON into an Edit proto.
    #[error("Deserialization error: {0}")]
    #[allow(dead_code)] // Used by PostgreSQL backend (not yet implemented)
    DeserializeError(String),
}

/// A cached edit entry with metadata.
///
/// This struct captures the various states a cache entry can be in:
/// - Successfully cached with edit content
/// - Errored (IPFS fetch failed, invalid CID, etc.)
/// - Missing edit content (entry exists but couldn't be decoded)
#[derive(Clone, Debug)]
pub struct CachedEdit {
    /// The IPFS CID/hash that was looked up.
    #[allow(dead_code)] // Part of public API for future use
    pub cid: String,

    /// The deserialized edit content, if available.
    /// Will be `None` if the entry is errored or couldn't be decoded.
    pub edit: Option<Edit>,

    /// Whether the cache entry is marked as errored.
    /// This happens when the IPFS cache service failed to fetch the content
    /// (e.g., invalid CID, IPFS gateway timeout, content not available).
    pub is_errored: bool,

    /// The space ID that published this edit (from the action).
    #[allow(dead_code)] // Part of public API for future use
    pub space_id: Vec<u8>,
}

impl CachedEdit {
    /// Create a successful cache entry with edit content.
    pub fn success(cid: String, edit: Edit, space_id: Vec<u8>) -> Self {
        Self {
            cid,
            edit: Some(edit),
            is_errored: false,
            space_id,
        }
    }

    /// Create an errored cache entry.
    pub fn errored(cid: String, space_id: Vec<u8>) -> Self {
        Self {
            cid,
            edit: None,
            is_errored: true,
            space_id,
        }
    }

    /// Check if this entry has valid edit content.
    #[allow(dead_code)] // Used in tests and future use
    pub fn has_content(&self) -> bool {
        !self.is_errored && self.edit.is_some()
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
    async fn get(&self, ipfs_hash: &str, space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        (**self).get(ipfs_hash, space_id).await
    }

    async fn get_batch(&self, requests: &[(&str, &[u8])]) -> Vec<Result<CachedEdit, CacheError>> {
        (**self).get_batch(requests).await
    }
}
