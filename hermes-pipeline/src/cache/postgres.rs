//! PostgreSQL-backed IPFS cache implementation.
//!
//! Reads from the `ipfs_cache` table populated by `hermes-ipfs-cache`.
//!
//! Note: As of v2, the cache stores raw GRC2/GRC2Z payload bytes that have
//! been validated by hermes-ipfs-cache. No decoding is performed here.

use std::env;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::{Postgres, Row, postgres::PgPoolOptions};

use super::{CacheError, CachedEdit, IpfsCache};

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// PostgreSQL-backed IPFS cache.
///
/// Connects to the same database as `hermes-ipfs-cache` to read pre-fetched
/// and validated IPFS content. The cache is keyed by IPFS URI (e.g., "ipfs://Qm...").
///
/// The stored bytes are raw GRC2/GRC2Z format, already validated by hermes-ipfs-cache.
/// Consumers (e.g., kg-indexer) must decode using the grc-20 crate.
pub struct PostgresCache {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresCache {
    /// Create a new cache connected to the given database.
    pub async fn new(database_url: &str) -> Result<Self, CacheError> {
        let pool = PgPoolOptions::new()
            .max_connections(env_or("PG_POOL_MAX", 20))
            // Close idle connections after 30s to free PgBouncer slots.
            .idle_timeout(Duration::from_secs(env_or("PG_IDLE_TIMEOUT_SECS", 30)))
            // Fail fast when pool is saturated — 3s means all 20 connections are busy,
            // indicating DB trouble, not normal load (max observed query time ~6s).
            .acquire_timeout(Duration::from_secs(env_or("PG_ACQUIRE_TIMEOUT_SECS", 3)))
            .connect(database_url)
            .await
            .map_err(|e| CacheError::Database(e.to_string()))?;

        Ok(PostgresCache { pool })
    }
}

#[async_trait]
impl IpfsCache for PostgresCache {
    async fn get(&self, ipfs_hash: &str, _space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        let row =
            sqlx::query("SELECT data, space, is_errored, name FROM ipfs_cache WHERE uri = $1")
                .bind(ipfs_hash)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| CacheError::Database(e.to_string()))?;

        match row {
            Some(row) => {
                let is_errored: bool = row.get("is_errored");
                let db_space_id: sqlx::types::Uuid = row.get("space");
                let space_id_bytes = db_space_id.as_bytes().to_vec();

                if is_errored {
                    return Ok(CachedEdit::errored(ipfs_hash.to_string(), space_id_bytes));
                }

                let data: Option<Vec<u8>> = row.get("data");
                let name: Option<String> = row.get("name");
                match data {
                    Some(payload) => {
                        // Return raw bytes - already validated by hermes-ipfs-cache
                        Ok(CachedEdit::success(
                            ipfs_hash.to_string(),
                            payload,
                            space_id_bytes,
                            name,
                        ))
                    }
                    None => {
                        // Entry exists but no data - treat as errored
                        Ok(CachedEdit::errored(ipfs_hash.to_string(), space_id_bytes))
                    }
                }
            }
            None => Err(CacheError::NotFound(ipfs_hash.to_string())),
        }
    }
}
