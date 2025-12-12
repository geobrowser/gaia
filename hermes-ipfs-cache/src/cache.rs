//! IPFS cache storage implementation.
//!
//! Stores resolved IPFS content in PostgreSQL so that downstream consumers
//! (like the edits transformer) don't need to block on network I/O.

use std::env;

use sqlx::{postgres::PgPoolOptions, Postgres};
use thiserror::Error;
use wire::pb::grc20::Edit;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    SerializeError(#[from] serde_json::Error),
}

/// PostgreSQL storage backend for the IPFS cache.
pub struct Storage {
    connection: sqlx::Pool<Postgres>,
}

impl Storage {
    /// Create a new storage instance connected to the database.
    pub async fn new() -> Result<Self, CacheError> {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL not set");

        let connection = PgPoolOptions::new()
            .max_connections(20)
            .connect(&database_url)
            .await?;

        Ok(Storage { connection })
    }

    /// Insert a cache item into storage, skipping if URI already exists.
    pub async fn insert(&self, item: &CacheItem) -> Result<(), CacheError> {
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

    /// Load the cursor for a given indexer ID.
    pub async fn load_cursor(&self, id: &str) -> Result<Option<String>, CacheError> {
        let result = sqlx::query_scalar::<_, String>("SELECT cursor FROM meta WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.connection)
            .await?;

        Ok(result)
    }

    /// Persist the cursor for a given indexer ID.
    pub async fn persist_cursor(
        &self,
        id: &str,
        cursor: &str,
        block: u64,
    ) -> Result<(), CacheError> {
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

/// High-level cache interface wrapping storage.
pub struct Cache {
    storage: Storage,
}

impl Cache {
    /// Create a new cache with the given storage backend.
    pub fn new(storage: Storage) -> Self {
        Cache { storage }
    }

    /// Store an item in the cache. If the URI already exists, this is a no-op.
    pub async fn put(&self, item: &CacheItem) -> Result<(), CacheError> {
        self.storage.insert(item).await
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
