//! PostgreSQL-backed IPFS cache implementation.
//!
//! Reads from the `ipfs_cache` table populated by `hermes-ipfs-cache`.

use async_trait::async_trait;
use prost::Message;
use sqlx::{Postgres, Row, postgres::PgPoolOptions};
use wire::pb::grc20::Edit;

use super::{CacheError, CachedEdit, IpfsCache};

/// PostgreSQL-backed IPFS cache.
///
/// Connects to the same database as `hermes-ipfs-cache` to read pre-fetched
/// IPFS content. The cache is keyed by IPFS URI (e.g., "ipfs://Qm...").
pub struct PostgresCache {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresCache {
    /// Create a new cache connected to the given database.
    pub async fn new(database_url: &str) -> Result<Self, CacheError> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url)
            .await
            .map_err(|e| CacheError::Database(e.to_string()))?;

        Ok(PostgresCache { pool })
    }
}

#[async_trait]
impl IpfsCache for PostgresCache {
    async fn get(&self, ipfs_hash: &str, _space_id: &[u8]) -> Result<CachedEdit, CacheError> {
        let row = sqlx::query("SELECT data, space, is_errored FROM ipfs_cache WHERE uri = $1")
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
                match data {
                    Some(bytes) => {
                        let edit = Edit::decode(bytes.as_slice())
                            .map_err(|e| CacheError::DeserializeError(e.to_string()))?;
                        Ok(CachedEdit::success(
                            ipfs_hash.to_string(),
                            edit,
                            space_id_bytes,
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
