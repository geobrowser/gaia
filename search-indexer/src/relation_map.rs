//! SQLite-backed relation-to-entity-space mapping with LRU cache.
//!
//! Maintains a local mapping of `relation_id → (entity_id, space_id)` so that
//! `DeleteRelation` events can resolve the target OpenSearch document without
//! querying Postgres (which may have already deleted the relation).
//!
//! Three-tier lookup: LRU cache → SQLite → Postgres fallback (existing path).
//! All errors are handled gracefully — SQLite failures never crash the indexer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use hermes_instrumentation::{error, info, warn};
use lru::LruCache;
use rusqlite::{params, Connection, OpenFlags};
use thiserror::Error;
use uuid::Uuid;

/// Errors from the relation map module.
#[derive(Debug, Error)]
pub enum RelationMapError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),
    #[error("Schema migration failed: {0}")]
    Migration(String),
    #[error("Postgres rebuild failed: {0}")]
    Rebuild(String),
}

/// Current schema version. Increment when making breaking changes.
const SCHEMA_VERSION: i64 = 1;

/// Configuration for the relation map.
#[derive(Debug, Clone)]
pub struct RelationMapConfig {
    pub db_path: PathBuf,
    pub cache_size: usize,
}

impl RelationMapConfig {
    pub fn from_env() -> Self {
        Self {
            db_path: PathBuf::from(
                std::env::var("RELATION_MAP_DB_PATH")
                    .unwrap_or_else(|_| "/data/relation_map.sqlite".to_string()),
            ),
            cache_size: std::env::var("RELATION_MAP_CACHE_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(500_000),
        }
    }
}

/// Atomic counters for observability.
pub struct RelationMapMetrics {
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub sqlite_hits: AtomicU64,
    pub sqlite_misses: AtomicU64,
    pub sqlite_errors: AtomicU64,
    pub inserts: AtomicU64,
    pub removes: AtomicU64,
}

impl RelationMapMetrics {
    fn new() -> Self {
        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            sqlite_hits: AtomicU64::new(0),
            sqlite_misses: AtomicU64::new(0),
            sqlite_errors: AtomicU64::new(0),
            inserts: AtomicU64::new(0),
            removes: AtomicU64::new(0),
        }
    }
}

/// Snapshot of metrics for logging.
#[derive(Debug)]
pub struct MetricsSnapshot {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub sqlite_hits: u64,
    pub sqlite_misses: u64,
    pub sqlite_errors: u64,
    pub inserts: u64,
    pub removes: u64,
    pub cache_len: usize,
    pub db_size_bytes: u64,
}

/// SQLite-backed relation map with LRU cache.
pub struct RelationMap {
    cache: Mutex<LruCache<Uuid, (Uuid, Uuid)>>,
    db: Mutex<Connection>,
    metrics: RelationMapMetrics,
    config: RelationMapConfig,
    needs_rebuild: bool,
}

impl RelationMap {
    /// Open or create the SQLite database. Enables WAL mode, validates schema,
    /// warms the LRU cache. Returns error only if SQLite is completely unusable.
    pub fn open(config: RelationMapConfig) -> Result<Self, RelationMapError> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let (conn, needs_rebuild) = match Self::open_and_validate(&config.db_path) {
            Ok(conn) => (conn, false),
            Err(e) => {
                warn!(
                    error = %e,
                    path = %config.db_path.display(),
                    "SQLite integrity check failed — recreating database"
                );
                // Delete corrupt file and recreate
                let _ = std::fs::remove_file(&config.db_path);
                let conn = Self::create_fresh(&config.db_path)?;
                (conn, true)
            }
        };

        let cache_size = std::num::NonZeroUsize::new(config.cache_size)
            .unwrap_or(std::num::NonZeroUsize::new(1).expect("1 is nonzero"));

        let mut map = Self {
            cache: Mutex::new(LruCache::new(cache_size)),
            db: Mutex::new(conn),
            metrics: RelationMapMetrics::new(),
            config,
            needs_rebuild,
        };

        // Warm cache from SQLite (best-effort)
        match map.warm_cache() {
            Ok(count) => {
                info!(
                    entries = count,
                    cache_size = map.config.cache_size,
                    "Relation map cache warmed from SQLite"
                );
            }
            Err(e) => {
                warn!(error = %e, "Failed to warm relation map cache");
            }
        }

        Ok(map)
    }

    /// Open an existing database, run integrity check, ensure schema is current.
    fn open_and_validate(path: &Path) -> Result<Connection, RelationMapError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        // Integrity check (limited to first result for speed)
        let integrity: String =
            conn.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
        if integrity != "ok" {
            return Err(RelationMapError::Migration(format!(
                "Integrity check failed: {}",
                integrity
            )));
        }

        Self::ensure_schema(&conn)?;

        Ok(conn)
    }

    /// Create a fresh database with the current schema.
    fn create_fresh(path: &Path) -> Result<Connection, RelationMapError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::ensure_schema(&conn)?;
        Ok(conn)
    }

    /// Create tables and set schema version.
    fn ensure_schema(conn: &Connection) -> Result<(), RelationMapError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS relation_map (
                 relation_id BLOB NOT NULL PRIMARY KEY,
                 entity_id BLOB NOT NULL,
                 space_id BLOB NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_relation_map_created_at
                 ON relation_map(created_at DESC);",
        )?;

        let version: Option<i64> = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .ok();

        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }
            Some(v) if v < SCHEMA_VERSION => {
                conn.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![SCHEMA_VERSION],
                )?;
                info!(
                    from_version = v,
                    to_version = SCHEMA_VERSION,
                    "Relation map schema migrated"
                );
            }
            _ => {}
        }

        Ok(())
    }

    /// Load the most recent entries from SQLite into the LRU cache.
    fn warm_cache(&mut self) -> Result<usize, RelationMapError> {
        let db = self
            .db
            .lock()
            .map_err(|e| RelationMapError::LockPoisoned(e.to_string()))?;
        let mut stmt = db.prepare(
            "SELECT relation_id, entity_id, space_id FROM relation_map
                 ORDER BY created_at DESC LIMIT ?1",
        )?;

        let mut cache = self
            .cache
            .lock()
            .map_err(|e| RelationMapError::LockPoisoned(e.to_string()))?;
        let mut count = 0usize;

        let rows = stmt.query_map(params![self.config.cache_size as i64], |row| {
            let rid: Vec<u8> = row.get(0)?;
            let eid: Vec<u8> = row.get(1)?;
            let sid: Vec<u8> = row.get(2)?;
            Ok((rid, eid, sid))
        })?;

        for row in rows {
            match row {
                Ok((rid, eid, sid)) => {
                    if let (Ok(r), Ok(e), Ok(s)) = (
                        Uuid::from_slice(&rid),
                        Uuid::from_slice(&eid),
                        Uuid::from_slice(&sid),
                    ) {
                        cache.put(r, (e, s));
                        count += 1;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Skipping corrupt row during cache warm");
                }
            }
        }

        Ok(count)
    }

    /// Whether the database needs a full rebuild from Postgres.
    pub fn needs_rebuild(&self) -> bool {
        self.needs_rebuild
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &RelationMapConfig {
        &self.config
    }

    /// Insert a relation mapping. Writes to both LRU cache and SQLite.
    /// Never fails — logs errors and continues.
    pub fn insert(&self, relation_id: Uuid, entity_id: Uuid, space_id: Uuid) {
        // Write to LRU cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.put(relation_id, (entity_id, space_id));
        }

        // Write to SQLite
        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.execute(
                "INSERT OR REPLACE INTO relation_map (relation_id, entity_id, space_id)
                 VALUES (?1, ?2, ?3)",
                params![
                    relation_id.as_bytes().as_slice(),
                    entity_id.as_bytes().as_slice(),
                    space_id.as_bytes().as_slice(),
                ],
            ) {
                error!(
                    error = %e,
                    relation_id = %relation_id,
                    "Failed to insert into relation map SQLite"
                );
                self.metrics.sqlite_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }

        self.metrics.inserts.fetch_add(1, Ordering::Relaxed);
    }

    /// Look up a relation mapping. Checks LRU cache first, then SQLite.
    /// Returns None if not found in either or on error.
    pub fn lookup(&self, relation_id: &Uuid) -> Option<(Uuid, Uuid)> {
        // Check LRU cache first
        if let Ok(mut cache) = self.cache.lock() {
            if let Some(&(entity_id, space_id)) = cache.get(relation_id) {
                self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some((entity_id, space_id));
            }
        }
        self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);

        // Fall back to SQLite
        let result = self.lookup_sqlite(relation_id);
        match &result {
            Some((entity_id, space_id)) => {
                self.metrics.sqlite_hits.fetch_add(1, Ordering::Relaxed);
                // Promote to LRU cache
                if let Ok(mut cache) = self.cache.lock() {
                    cache.put(*relation_id, (*entity_id, *space_id));
                }
            }
            None => {
                self.metrics.sqlite_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }

    /// SQLite lookup by primary key.
    fn lookup_sqlite(&self, relation_id: &Uuid) -> Option<(Uuid, Uuid)> {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(e) => {
                error!(error = %e, "Relation map SQLite lock poisoned");
                self.metrics.sqlite_errors.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };

        match db.query_row(
            "SELECT entity_id, space_id FROM relation_map WHERE relation_id = ?1",
            params![relation_id.as_bytes().as_slice()],
            |row| {
                let eid: Vec<u8> = row.get(0)?;
                let sid: Vec<u8> = row.get(1)?;
                Ok((eid, sid))
            },
        ) {
            Ok((eid, sid)) => match (Uuid::from_slice(&eid), Uuid::from_slice(&sid)) {
                (Ok(e), Ok(s)) => Some((e, s)),
                _ => {
                    warn!(relation_id = %relation_id, "Corrupt UUID in relation map");
                    None
                }
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => {
                error!(error = %e, "Relation map SQLite lookup failed");
                self.metrics.sqlite_errors.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Remove a relation mapping from both cache and SQLite.
    pub fn remove(&self, relation_id: &Uuid) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.pop(relation_id);
        }

        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.execute(
                "DELETE FROM relation_map WHERE relation_id = ?1",
                params![relation_id.as_bytes().as_slice()],
            ) {
                error!(error = %e, "Failed to remove from relation map SQLite");
                self.metrics.sqlite_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }

        self.metrics.removes.fetch_add(1, Ordering::Relaxed);
    }

    /// Flush SQLite WAL to main database file. Call on shutdown.
    pub fn checkpoint(&self) {
        if let Ok(db) = self.db.lock() {
            if let Err(e) = db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)") {
                warn!(error = %e, "Failed to checkpoint relation map SQLite");
            }
        }
    }

    /// Get current metrics snapshot for logging.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        let cache_len = self.cache.lock().map(|c| c.len()).unwrap_or(0);

        MetricsSnapshot {
            cache_hits: self.metrics.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.metrics.cache_misses.load(Ordering::Relaxed),
            sqlite_hits: self.metrics.sqlite_hits.load(Ordering::Relaxed),
            sqlite_misses: self.metrics.sqlite_misses.load(Ordering::Relaxed),
            sqlite_errors: self.metrics.sqlite_errors.load(Ordering::Relaxed),
            inserts: self.metrics.inserts.load(Ordering::Relaxed),
            removes: self.metrics.removes.load(Ordering::Relaxed),
            cache_len,
            db_size_bytes: self.db_size_bytes(),
        }
    }

    /// Get SQLite file size in bytes.
    pub fn db_size_bytes(&self) -> u64 {
        std::fs::metadata(&self.config.db_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Get the total number of entries in SQLite.
    pub fn sqlite_count(&self) -> u64 {
        let db = match self.db.lock() {
            Ok(db) => db,
            Err(_) => return 0,
        };
        db.query_row("SELECT count(*) FROM relation_map", [], |row| row.get(0))
            .unwrap_or(0i64) as u64
    }

    /// Rebuild the SQLite database from a Postgres connection pool.
    /// Uses cursor-based pagination to avoid loading the entire table into memory.
    pub async fn rebuild_from_postgres(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<u64, RelationMapError> {
        const BATCH_SIZE: i64 = 10_000;

        info!("Rebuilding relation map from Postgres (batch_size={BATCH_SIZE})...");

        let mut total: u64 = 0;
        let mut last_id: Option<Uuid> = None;

        loop {
            let rows: Vec<(Uuid, Uuid, Uuid)> = match last_id {
                None => {
                    sqlx::query_as(
                        "SELECT id, entity_id, space_id FROM relations ORDER BY id LIMIT $1",
                    )
                    .bind(BATCH_SIZE)
                    .fetch_all(pool)
                    .await
                }
                Some(cursor) => {
                    sqlx::query_as(
                        "SELECT id, entity_id, space_id FROM relations WHERE id > $1 ORDER BY id LIMIT $2",
                    )
                    .bind(cursor)
                    .bind(BATCH_SIZE)
                    .fetch_all(pool)
                    .await
                }
            }
            .map_err(|e| RelationMapError::Rebuild(format!("Postgres query failed: {}", e)))?;

            let batch_len = rows.len() as u64;
            if batch_len == 0 {
                break;
            }

            last_id = rows.last().map(|(id, _, _)| *id);

            let mut db = self
                .db
                .lock()
                .map_err(|e| RelationMapError::LockPoisoned(e.to_string()))?;

            let tx = db.transaction()?;

            for (relation_id, entity_id, space_id) in &rows {
                if let Err(e) = tx.execute(
                    "INSERT OR REPLACE INTO relation_map (relation_id, entity_id, space_id)
                     VALUES (?1, ?2, ?3)",
                    params![
                        relation_id.as_bytes().as_slice(),
                        entity_id.as_bytes().as_slice(),
                        space_id.as_bytes().as_slice(),
                    ],
                ) {
                    warn!(error = %e, "Failed to insert during rebuild");
                }
            }

            tx.commit()?;

            total += batch_len;

            if total.is_multiple_of(100_000) || batch_len < BATCH_SIZE as u64 {
                info!(
                    progress = total,
                    batch = batch_len,
                    "Relation map rebuild progress"
                );
            }

            // Last batch was smaller than limit — we're done
            if batch_len < BATCH_SIZE as u64 {
                break;
            }
        }

        info!(total = total, "Relation map rebuilt from Postgres");

        Ok(total)
    }

    /// Log a heartbeat with current metrics. Call periodically from the orchestrator.
    pub fn log_heartbeat(&self) {
        let m = self.metrics_snapshot();
        let hit_rate = if m.cache_hits + m.cache_misses > 0 {
            (m.cache_hits as f64 / (m.cache_hits + m.cache_misses) as f64) * 100.0
        } else {
            0.0
        };

        let db_size_mb = m.db_size_bytes as f64 / 1_048_576.0;

        info!(
            cache_hits = m.cache_hits,
            cache_misses = m.cache_misses,
            cache_hit_rate_pct = hit_rate,
            cache_len = m.cache_len,
            sqlite_hits = m.sqlite_hits,
            sqlite_misses = m.sqlite_misses,
            sqlite_errors = m.sqlite_errors,
            inserts = m.inserts,
            removes = m.removes,
            db_size_mb = db_size_mb,
            "Relation map heartbeat"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config(dir: &tempfile::TempDir) -> RelationMapConfig {
        RelationMapConfig {
            db_path: dir.path().join("test_relation_map.sqlite"),
            cache_size: 100,
        }
    }

    #[test]
    fn test_open_creates_fresh_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");
        assert!(!map.needs_rebuild());
        assert_eq!(map.sqlite_count(), 0);
    }

    #[test]
    fn test_insert_and_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let rid = Uuid::new_v4();
        let eid = Uuid::new_v4();
        let sid = Uuid::new_v4();

        map.insert(rid, eid, sid);

        let result = map.lookup(&rid);
        assert_eq!(result, Some((eid, sid)));
        assert_eq!(map.sqlite_count(), 1);
    }

    #[test]
    fn test_lookup_miss() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let result = map.lookup(&Uuid::new_v4());
        assert_eq!(result, None);
    }

    #[test]
    fn test_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let rid = Uuid::new_v4();
        let eid = Uuid::new_v4();
        let sid = Uuid::new_v4();

        map.insert(rid, eid, sid);
        assert!(map.lookup(&rid).is_some());

        map.remove(&rid);
        assert_eq!(map.lookup(&rid), None);
        assert_eq!(map.sqlite_count(), 0);
    }

    #[test]
    fn test_sqlite_fallback_after_cache_eviction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(&dir);
        config.cache_size = 2; // Tiny cache to force eviction
        let map = RelationMap::open(config).expect("open");

        let rid1 = Uuid::new_v4();
        let rid2 = Uuid::new_v4();
        let rid3 = Uuid::new_v4();
        let eid = Uuid::new_v4();
        let sid = Uuid::new_v4();

        // Insert 3 entries, cache holds 2
        map.insert(rid1, eid, sid);
        map.insert(rid2, eid, sid);
        map.insert(rid3, eid, sid);

        // rid1 was evicted from cache but should be in SQLite
        let result = map.lookup(&rid1);
        assert_eq!(result, Some((eid, sid)));

        let m = map.metrics_snapshot();
        assert!(m.sqlite_hits > 0, "Should have hit SQLite");
    }

    #[test]
    fn test_warm_cache_on_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);

        let rid = Uuid::new_v4();
        let eid = Uuid::new_v4();
        let sid = Uuid::new_v4();

        // Insert and close
        {
            let map = RelationMap::open(config.clone()).expect("open");
            map.insert(rid, eid, sid);
            map.checkpoint();
        }

        // Reopen — cache should be warmed from SQLite
        {
            let map = RelationMap::open(config).expect("reopen");
            let result = map.lookup(&rid);
            assert_eq!(result, Some((eid, sid)));

            // Should be a cache hit (warmed from SQLite on open)
            let m = map.metrics_snapshot();
            assert_eq!(m.cache_hits, 1);
            assert_eq!(m.sqlite_hits, 0);
        }
    }

    #[test]
    fn test_integrity_check_failure_recreates_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);

        // Write garbage to the SQLite file
        std::fs::write(&config.db_path, b"this is not a sqlite file").expect("write");

        // Open should detect corruption and recreate
        let map = RelationMap::open(config).expect("open after corruption");
        assert!(map.needs_rebuild());
        assert_eq!(map.sqlite_count(), 0);
    }

    #[test]
    fn test_metrics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let rid = Uuid::new_v4();
        map.insert(rid, Uuid::new_v4(), Uuid::new_v4());
        map.lookup(&rid); // cache hit
        map.lookup(&Uuid::new_v4()); // cache miss + sqlite miss

        let m = map.metrics_snapshot();
        assert_eq!(m.inserts, 1);
        assert_eq!(m.cache_hits, 1);
        assert_eq!(m.cache_misses, 1);
        assert_eq!(m.sqlite_misses, 1);
    }

    #[test]
    fn test_insert_or_replace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let rid = Uuid::new_v4();
        let eid1 = Uuid::new_v4();
        let eid2 = Uuid::new_v4();
        let sid = Uuid::new_v4();

        map.insert(rid, eid1, sid);
        map.insert(rid, eid2, sid); // Should replace

        let result = map.lookup(&rid);
        assert_eq!(result, Some((eid2, sid)));
        assert_eq!(map.sqlite_count(), 1); // Still 1, not 2
    }
}
