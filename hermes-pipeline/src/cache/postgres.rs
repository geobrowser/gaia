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

/// `meta.block_number` is stored as text by every indexer that writes `meta`.
fn parse_block_number(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// `meta.id` under which hermes-ipfs-cache persists its cursor. Must match the
/// indexer id the warmer writes (see hermes-ipfs-cache's cursor store).
const IPFS_CACHE_INDEXER_ID: &str = "hermes_ipfs_cache";

#[async_trait]
impl IpfsCache for PostgresCache {
    /// Reads the warmer's durable cursor from the shared `meta` table. On any
    /// error this returns `None` — "unknown", which callers treat as "not yet
    /// processed" so a transient DB blip can never be mistaken for a verdict
    /// that content is unfetchable.
    ///
    /// `meta.block_number` is TEXT. This used to decode it as `i64`; the type
    /// mismatch was swallowed by `.ok()`, so from #845 until 2026-10-05 this always
    /// returned `None` and every unfetchable URI held the pipeline for the full
    /// `warmer_wait_max` backstop (a ~2h indexer stall that day). Failures are now
    /// logged, so a wrong answer here can never again look like a slow warmer.
    async fn warmer_block(&self) -> Option<u64> {
        let row = sqlx::query_scalar::<_, String>("SELECT block_number FROM meta WHERE id = $1")
            .bind(IPFS_CACHE_INDEXER_ID)
            .fetch_optional(&self.pool)
            .await;
        match row {
            Ok(Some(text)) => {
                let parsed = parse_block_number(&text);
                if parsed.is_none() {
                    tracing::warn!(value = %text, "IPFS warmer cursor block_number is not a block number");
                }
                parsed
            }
            Ok(None) => {
                tracing::warn!(
                    id = IPFS_CACHE_INDEXER_ID,
                    "IPFS warmer has no cursor row in meta"
                );
                None
            }
            Err(error) => {
                tracing::warn!(error = %error, "Could not read the IPFS warmer cursor");
                None
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_text_block_numbers_meta_stores() {
        assert_eq!(parse_block_number("72441"), Some(72441));
        assert_eq!(parse_block_number(" 72441\n"), Some(72441));
        assert_eq!(parse_block_number(""), None);
        assert_eq!(parse_block_number("-1"), None);
        assert_eq!(parse_block_number("abc"), None);
    }

    /// The 2026-10-05 regression, against a real Postgres: `meta.block_number` is
    /// TEXT, and reading it must yield the warmer's block. Ignored by default because
    /// it needs a database:
    ///   DATABASE_URL=postgres://… cargo test -p hermes-pipeline -- --ignored warmer_block_reads
    #[tokio::test]
    #[ignore = "needs DATABASE_URL pointing at a scratch Postgres"]
    async fn warmer_block_reads_the_text_cursor_meta_stores() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let cache = PostgresCache::new(&url).await.expect("connect");
        sqlx::query("CREATE TABLE IF NOT EXISTS meta (id text PRIMARY KEY, cursor text NOT NULL, block_number text NOT NULL)")
            .execute(&cache.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO meta (id, cursor, block_number) VALUES ($1, 'c', '72441') ON CONFLICT (id) DO UPDATE SET block_number = '72441'")
            .bind(IPFS_CACHE_INDEXER_ID)
            .execute(&cache.pool)
            .await
            .unwrap();
        assert_eq!(cache.warmer_block().await, Some(72441));
    }
}
