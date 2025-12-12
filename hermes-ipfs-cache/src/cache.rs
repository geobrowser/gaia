//! IPFS cache storage implementation.
//!
//! Stores resolved IPFS content so that downstream consumers
//! (like the edits transformer) don't need to block on network I/O.
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_ipfs_cache::cache::CacheSource;
//!
//! // Development: use in-memory cache
//! let cache = CacheSource::mock().into_cache().await?;
//!
//! // Production: use PostgreSQL
//! let cache = CacheSource::live("postgres://...").into_cache().await?;
//! ```

use std::collections::HashMap;
use std::sync::RwLock;

use thiserror::Error;
use wire::pb::grc20::Edit;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    SerializeError(#[from] serde_json::Error),
}

/// A cached IPFS content item.
pub struct CacheItem {
    /// The IPFS URI (e.g., "ipfs://Qm...")
    pub uri: String,
    /// Decoded edit content, if successfully fetched and decoded
    pub json: Option<Edit>,
    /// Block timestamp when this was cached
    pub block: String,
    /// Space ID (16 bytes, hex-encoded)
    pub space_id: String,
    /// Whether fetching/decoding failed
    pub is_errored: bool,
}

/// Configuration for the cache storage backend.
///
/// Use this to explicitly choose between mock (in-memory) and live (PostgreSQL) storage,
/// following the same pattern as `StreamSource` and `IpfsSource`.
#[derive(Debug, Clone)]
pub enum CacheSource {
    /// Use in-memory cache for testing/development.
    Mock,

    /// Use PostgreSQL storage.
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
    pub async fn into_cache(self) -> Result<Cache, CacheError> {
        match self {
            Self::Mock => Ok(Cache::mock()),
            Self::Live { database_url } => {
                let storage = PostgresStorage::new(&database_url).await?;
                Ok(Cache::postgres(storage))
            }
        }
    }
}

/// Trait for cache storage backends.
#[async_trait::async_trait]
pub trait CacheStorage: Send + Sync {
    /// Insert a cache item, skipping if URI already exists.
    async fn insert(&self, item: &CacheItem) -> Result<(), CacheError>;

    /// Get a cache item by URI.
    async fn get(&self, uri: &str) -> Result<Option<CacheItem>, CacheError>;

    /// Load the cursor for a given indexer ID.
    async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError>;

    /// Persist the cursor for a given indexer ID.
    async fn persist_cursor(&self, id: &str, cursor: &str, block: u64) -> Result<(), CacheError>;
}

// =============================================================================
// Mock (In-Memory) Storage
// =============================================================================

/// In-memory storage backend for testing/development.
pub struct MockStorage {
    items: RwLock<HashMap<String, StoredItem>>,
    cursors: RwLock<HashMap<String, (String, u64)>>,
}

/// Internal representation of a stored cache item.
struct StoredItem {
    json: Option<Edit>,
    block: String,
    space_id: String,
    is_errored: bool,
}

impl MockStorage {
    pub fn new() -> Self {
        Self {
            items: RwLock::new(HashMap::new()),
            cursors: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MockStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CacheStorage for MockStorage {
    async fn insert(&self, item: &CacheItem) -> Result<(), CacheError> {
        let mut items = self.items.write().unwrap();
        // Only insert if not exists (matches PostgreSQL ON CONFLICT DO NOTHING)
        if !items.contains_key(&item.uri) {
            items.insert(
                item.uri.clone(),
                StoredItem {
                    json: item.json.clone(),
                    block: item.block.clone(),
                    space_id: item.space_id.clone(),
                    is_errored: item.is_errored,
                },
            );
        }
        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Option<CacheItem>, CacheError> {
        let items = self.items.read().unwrap();
        Ok(items.get(uri).map(|stored| CacheItem {
            uri: uri.to_string(),
            json: stored.json.clone(),
            block: stored.block.clone(),
            space_id: stored.space_id.clone(),
            is_errored: stored.is_errored,
        }))
    }

    async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError> {
        let cursors = self.cursors.read().unwrap();
        Ok(cursors.get(id).map(|(cursor, _)| cursor.clone()))
    }

    async fn persist_cursor(&self, id: &str, cursor: &str, block: u64) -> Result<(), CacheError> {
        let mut cursors = self.cursors.write().unwrap();
        cursors.insert(id.to_string(), (cursor.to_string(), block));
        Ok(())
    }
}

// =============================================================================
// PostgreSQL Storage
// =============================================================================

use sqlx::{postgres::PgPoolOptions, Postgres};

/// PostgreSQL storage backend for the IPFS cache.
pub struct PostgresStorage {
    connection: sqlx::Pool<Postgres>,
}

impl PostgresStorage {
    /// Create a new storage instance connected to the database.
    pub async fn new(database_url: &str) -> Result<Self, CacheError> {
        let connection = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url)
            .await?;

        Ok(PostgresStorage { connection })
    }
}

#[async_trait::async_trait]
impl CacheStorage for PostgresStorage {
    async fn insert(&self, item: &CacheItem) -> Result<(), CacheError> {
        let json_value = serde_json::to_value(&item.json)?;

        sqlx::query(
            "INSERT INTO ipfs_cache (uri, json, block, space_id, is_errored) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (uri) DO NOTHING",
        )
        .bind(&item.uri)
        .bind(&json_value)
        .bind(&item.block)
        .bind(&item.space_id)
        .bind(item.is_errored)
        .execute(&self.connection)
        .await?;

        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Option<CacheItem>, CacheError> {
        let row: Option<(serde_json::Value, String, String, bool)> = sqlx::query_as(
            "SELECT json, block, space_id, is_errored FROM ipfs_cache WHERE uri = $1",
        )
        .bind(uri)
        .fetch_optional(&self.connection)
        .await?;

        match row {
            Some((json_value, block, space_id, is_errored)) => {
                let json: Option<Edit> = serde_json::from_value(json_value)?;
                Ok(Some(CacheItem {
                    uri: uri.to_string(),
                    json,
                    block,
                    space_id,
                    is_errored,
                }))
            }
            None => Ok(None),
        }
    }

    async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError> {
        let result = sqlx::query_scalar::<_, String>("SELECT cursor FROM meta WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.connection)
            .await?;

        Ok(result)
    }

    async fn persist_cursor(&self, id: &str, cursor: &str, block: u64) -> Result<(), CacheError> {
        sqlx::query(
            "INSERT INTO meta (id, cursor, block_number) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET cursor = $2, block_number = $3",
        )
        .bind(id)
        .bind(cursor)
        .bind(block.to_string())
        .execute(&self.connection)
        .await?;

        Ok(())
    }
}

// =============================================================================
// Cache (High-Level Interface)
// =============================================================================

/// High-level cache interface wrapping a storage backend.
pub struct Cache {
    storage: Box<dyn CacheStorage>,
}

impl Cache {
    /// Create a cache with in-memory storage (for testing).
    pub fn mock() -> Self {
        Cache {
            storage: Box::new(MockStorage::new()),
        }
    }

    /// Create a cache with PostgreSQL storage.
    pub fn postgres(storage: PostgresStorage) -> Self {
        Cache {
            storage: Box::new(storage),
        }
    }

    /// Store an item in the cache. If the URI already exists, this is a no-op.
    pub async fn put(&self, item: &CacheItem) -> Result<(), CacheError> {
        self.storage.insert(item).await
    }

    /// Get an item from the cache by URI.
    pub async fn get(&self, uri: &str) -> Result<Option<CacheItem>, CacheError> {
        self.storage.get(uri).await
    }

    /// Load the cursor for a given indexer ID.
    pub async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError> {
        self.storage.load_cursor(id).await
    }

    /// Persist the cursor for a given indexer ID.
    pub async fn persist_cursor(
        &self,
        id: &str,
        cursor: &str,
        block: u64,
    ) -> Result<(), CacheError> {
        self.storage.persist_cursor(id, cursor, block).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_edit(name: &str) -> Edit {
        Edit {
            id: vec![0x01, 0x02],
            name: name.to_string(),
            ops: vec![],
            authors: vec![],
            language: None,
        }
    }

    #[tokio::test]
    async fn test_mock_cache_put_and_get() {
        let cache = Cache::mock();

        let item = CacheItem {
            uri: "ipfs://QmTest123".to_string(),
            json: Some(test_edit("Test Edit")),
            block: "12345".to_string(),
            space_id: "abc123".to_string(),
            is_errored: false,
        };

        cache.put(&item).await.unwrap();

        let retrieved = cache.get("ipfs://QmTest123").await.unwrap();
        assert!(retrieved.is_some());

        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.uri, "ipfs://QmTest123");
        assert_eq!(retrieved.json.unwrap().name, "Test Edit");
        assert_eq!(retrieved.block, "12345");
        assert_eq!(retrieved.space_id, "abc123");
        assert!(!retrieved.is_errored);
    }

    #[tokio::test]
    async fn test_mock_cache_put_duplicate_is_noop() {
        let cache = Cache::mock();

        let item1 = CacheItem {
            uri: "ipfs://QmTest123".to_string(),
            json: Some(test_edit("First")),
            block: "100".to_string(),
            space_id: "abc".to_string(),
            is_errored: false,
        };

        let item2 = CacheItem {
            uri: "ipfs://QmTest123".to_string(),
            json: Some(test_edit("Second")),
            block: "200".to_string(),
            space_id: "def".to_string(),
            is_errored: false,
        };

        cache.put(&item1).await.unwrap();
        cache.put(&item2).await.unwrap();

        // Should still have the first item
        let retrieved = cache.get("ipfs://QmTest123").await.unwrap().unwrap();
        assert_eq!(retrieved.json.unwrap().name, "First");
    }

    #[tokio::test]
    async fn test_mock_cache_get_nonexistent() {
        let cache = Cache::mock();

        let result = cache.get("ipfs://QmNotFound").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_mock_cache_cursor_persistence() {
        let cache = Cache::mock();

        // Initially no cursor
        let cursor = cache.load_cursor("test_indexer").await.unwrap();
        assert!(cursor.is_none());

        // Persist a cursor
        cache
            .persist_cursor("test_indexer", "cursor_abc", 100)
            .await
            .unwrap();

        // Should be able to load it
        let cursor = cache.load_cursor("test_indexer").await.unwrap();
        assert_eq!(cursor, Some("cursor_abc".to_string()));

        // Update the cursor
        cache
            .persist_cursor("test_indexer", "cursor_def", 200)
            .await
            .unwrap();

        let cursor = cache.load_cursor("test_indexer").await.unwrap();
        assert_eq!(cursor, Some("cursor_def".to_string()));
    }
}
