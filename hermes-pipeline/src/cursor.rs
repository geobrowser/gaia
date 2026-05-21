//! Cursor persistence for hermes-pipeline.
//!
//! Stores the last successfully-processed substreams cursor so the pipeline
//! can resume after restart instead of re-streaming from
//! `SUBSTREAMS_START_BLOCK`.
//!
//! Storage shape mirrors the `meta` table that `hermes-ipfs-cache` already
//! owns: `(id TEXT PK, cursor TEXT, block_number TEXT)`. We write rows with
//! `id = "hermes_pipeline"`; the cache writes with `id = "hermes_ipfs_cache"`.
//! The table is shared, the abstraction is not — the pipeline doesn't need
//! the full `IpfsCache` surface just to checkpoint itself.
//!
//! ## Usage
//!
//! ```ignore
//! use hermes_pipeline::cursor::{CursorStore, MockCursorStore, PostgresCursorStore};
//! use std::sync::Arc;
//!
//! // Development: in-memory store
//! let store: Arc<dyn CursorStore> = Arc::new(MockCursorStore::new());
//!
//! // Production: PostgreSQL
//! let store: Arc<dyn CursorStore> =
//!     Arc::new(PostgresCursorStore::new("postgres://...").await?);
//! ```

use std::env;
use std::sync::RwLock;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::{Postgres, postgres::PgPoolOptions};
use thiserror::Error;

/// Indexer ID for cursor persistence. Identifies the pipeline's row in the
/// shared `meta` table.
pub const INDEXER_ID: &str = "hermes_pipeline";

#[derive(Error, Debug)]
pub enum CursorStoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// Storage backend for the pipeline's substreams cursor.
#[async_trait]
pub trait CursorStore: Send + Sync {
    /// Load the persisted cursor. Returns `None` on cold start.
    async fn load(&self) -> Result<Option<String>, CursorStoreError>;

    /// Persist the cursor for the most recently completed block.
    async fn persist(&self, cursor: &str, block: u64) -> Result<(), CursorStoreError>;
}

// =============================================================================
// Mock (in-memory) store
// =============================================================================

/// In-memory cursor store for tests and mock-mode runs.
#[derive(Default)]
pub struct MockCursorStore {
    state: RwLock<Option<(String, u64)>>,
}

impl MockCursorStore {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(None),
        }
    }
}

#[async_trait]
impl CursorStore for MockCursorStore {
    async fn load(&self) -> Result<Option<String>, CursorStoreError> {
        let state = self.state.read().expect("MockCursorStore lock poisoned");
        Ok(state.as_ref().map(|(c, _)| c.clone()))
    }

    async fn persist(&self, cursor: &str, block: u64) -> Result<(), CursorStoreError> {
        let mut state = self.state.write().expect("MockCursorStore lock poisoned");
        *state = Some((cursor.to_string(), block));
        Ok(())
    }
}

// =============================================================================
// PostgreSQL store
// =============================================================================

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// PostgreSQL-backed cursor store.
///
/// Timeout knobs (idle / acquire) honor the same env vars `hermes-ipfs-cache`
/// reads — same DB, same connection-behavior expectations. `max_connections`
/// is **not** shared: cursor writes are ~1 per block (sequential, one persist
/// per successful block), so 2 connections covers the steady state plus retry
/// headroom. Inheriting `PG_POOL_MAX` (default 20) would double the pipeline
/// pod's Postgres footprint for no real concurrency.
const CURSOR_POOL_MAX_CONNECTIONS: u32 = 2;

pub struct PostgresCursorStore {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresCursorStore {
    pub async fn new(database_url: &str) -> Result<Self, CursorStoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(CURSOR_POOL_MAX_CONNECTIONS)
            // Close idle connections after 30s to free PgBouncer slots.
            .idle_timeout(Duration::from_secs(env_or("PG_IDLE_TIMEOUT_SECS", 30)))
            // Fail fast when pool is saturated.
            .acquire_timeout(Duration::from_secs(env_or("PG_ACQUIRE_TIMEOUT_SECS", 3)))
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl CursorStore for PostgresCursorStore {
    async fn load(&self) -> Result<Option<String>, CursorStoreError> {
        let cursor = sqlx::query_scalar::<_, String>("SELECT cursor FROM meta WHERE id = $1")
            .bind(INDEXER_ID)
            .fetch_optional(&self.pool)
            .await?;
        Ok(cursor)
    }

    async fn persist(&self, cursor: &str, block: u64) -> Result<(), CursorStoreError> {
        // block_number is TEXT in the meta table — match hermes-ipfs-cache's
        // binding convention rather than introducing a column-type mismatch.
        sqlx::query(
            "INSERT INTO meta (id, cursor, block_number) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET cursor = $2, block_number = $3",
        )
        .bind(INDEXER_ID)
        .bind(cursor)
        .bind(block.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_cold_start_returns_none() {
        let store = MockCursorStore::new();
        assert!(store.load().await.expect("load").is_none());
    }

    #[tokio::test]
    async fn mock_persist_then_load_roundtrips() {
        let store = MockCursorStore::new();
        store.persist("cursor_abc", 100).await.expect("persist");

        assert_eq!(
            store.load().await.expect("load"),
            Some("cursor_abc".to_string())
        );
    }

    #[tokio::test]
    async fn mock_persist_overwrites_previous() {
        let store = MockCursorStore::new();
        store.persist("cursor_abc", 100).await.expect("first");
        store.persist("cursor_def", 200).await.expect("second");

        assert_eq!(
            store.load().await.expect("load"),
            Some("cursor_def".to_string())
        );
    }

    /// Integration test against a real Postgres. Mirrors the ignored-by-default
    /// pattern in hermes-ipfs-cache: run via `DATABASE_URL=... cargo test
    /// -p hermes-pipeline -- --ignored postgres_cursor`.
    #[tokio::test]
    #[ignore]
    async fn postgres_persist_then_load_roundtrips() {
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let store = PostgresCursorStore::new(&database_url)
            .await
            .expect("connect");

        // Use a unique cursor each run so we can detect that this write
        // (not a prior one) was what we read back. We share `INDEXER_ID =
        // hermes_pipeline` with the live pipeline; the upsert semantics make
        // this fine for a manual integration test.
        let cursor = format!(
            "test_cursor_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        store.persist(&cursor, 12345).await.expect("persist");

        let loaded = store.load().await.expect("load");
        assert_eq!(loaded, Some(cursor));
    }
}
