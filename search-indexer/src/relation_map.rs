//! Embedded-store relation-to-entity-space mapping with LRU cache.
//!
//! Maintains a local mapping of `relation_id -> (entity_id, space_id)` so that
//! `DeleteRelation` events can resolve the target OpenSearch document without
//! querying Postgres (which may have already deleted the relation).
//!
//! Three-tier lookup: LRU cache -> on-disk store -> Postgres fallback (existing path).
//! The store is `redb`, a pure-Rust embedded database, so no C library (and no bundled
//! SQLite) is built into the binary. All errors are handled gracefully: store failures
//! never crash the indexer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use hermes_instrumentation::{error, info, warn};
use lru::LruCache;
use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};
use thiserror::Error;
use uuid::Uuid;

/// relation id -> entity id || space id (16 + 16 bytes).
const TABLE: TableDefinition<[u8; 16], [u8; 32]> = TableDefinition::new("relation_map");

/// Store bookkeeping. Holds `REBUILT_KEY` once a full rebuild from Postgres has finished.
const META: TableDefinition<&str, u64> = TableDefinition::new("relation_map_meta");

/// Present (value: rows written) only after `rebuild_from_postgres` ran to the end. A store
/// without it was never seeded, or its seeding was cut short (pod killed mid-rebuild), so it
/// holds only part of the relations and is rebuilt on the next open.
const REBUILT_KEY: &str = "rebuilt_from_postgres";

/// Errors from the relation map module.
#[derive(Debug, Error)]
pub enum RelationMapError {
    #[error("store error: {0}")]
    Store(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),
    #[error("Postgres rebuild failed: {0}")]
    Rebuild(String),
}

fn store_err(e: impl std::fmt::Display) -> RelationMapError {
    RelationMapError::Store(e.to_string())
}

fn pack(entity_id: &Uuid, space_id: &Uuid) -> [u8; 32] {
    let mut v = [0u8; 32];
    v[..16].copy_from_slice(entity_id.as_bytes());
    v[16..].copy_from_slice(space_id.as_bytes());
    v
}

fn unpack(v: &[u8; 32]) -> (Uuid, Uuid) {
    let mut e = [0u8; 16];
    let mut s = [0u8; 16];
    e.copy_from_slice(&v[..16]);
    s.copy_from_slice(&v[16..]);
    (Uuid::from_bytes(e), Uuid::from_bytes(s))
}

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
                    .unwrap_or_else(|_| "/data/relation_map.redb".to_string()),
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
    pub store_hits: AtomicU64,
    pub store_misses: AtomicU64,
    pub store_errors: AtomicU64,
    pub inserts: AtomicU64,
    pub removes: AtomicU64,
}

impl RelationMapMetrics {
    fn new() -> Self {
        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            store_hits: AtomicU64::new(0),
            store_misses: AtomicU64::new(0),
            store_errors: AtomicU64::new(0),
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
    pub store_hits: u64,
    pub store_misses: u64,
    pub store_errors: u64,
    pub inserts: u64,
    pub removes: u64,
    pub cache_len: usize,
    pub db_size_bytes: u64,
}

/// Embedded-store relation map with LRU cache.
pub struct RelationMap {
    cache: Mutex<LruCache<Uuid, (Uuid, Uuid)>>,
    db: Database,
    metrics: RelationMapMetrics,
    config: RelationMapConfig,
    needs_rebuild: bool,
}

impl RelationMap {
    /// Open or create the store and warm the LRU cache. A missing, unreadable or
    /// foreign file (including the old SQLite file at the same path) is replaced by a
    /// fresh store and `needs_rebuild()` is true, so the caller repopulates it from
    /// Postgres. So is a readable store whose last rebuild never finished: each rebuild
    /// batch is committed on its own, so a pod killed mid-rebuild leaves a valid store
    /// holding only the relations up to that batch. Returns error only if the store is
    /// completely unusable.
    pub fn open(config: RelationMapConfig) -> Result<Self, RelationMapError> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let (db, needs_rebuild) = match Self::open_store(&config) {
            Ok(db) => {
                // An unreadable marker counts as no completed rebuild.
                let rebuilt = Self::rebuilt_rows(&db).unwrap_or(None);
                if rebuilt.is_none() {
                    info!(
                        path = %config.db_path.display(),
                        "Relation map store has no completed rebuild; it will be rebuilt"
                    );
                }
                (db, rebuilt.is_none())
            }
            Err(e) => {
                warn!(
                    error = %e,
                    path = %config.db_path.display(),
                    "Relation map store unreadable; recreating it"
                );
                let _ = std::fs::remove_file(&config.db_path);
                (Self::open_store(&config)?, true)
            }
        };

        let cache_size = std::num::NonZeroUsize::new(config.cache_size)
            .unwrap_or(std::num::NonZeroUsize::new(1).expect("1 is nonzero"));

        let map = Self {
            cache: Mutex::new(LruCache::new(cache_size)),
            db,
            metrics: RelationMapMetrics::new(),
            config,
            needs_rebuild,
        };

        match map.warm_cache() {
            Ok(count) => info!(
                entries = count,
                cache_size = map.config.cache_size,
                "Relation map cache warmed from store"
            ),
            Err(e) => warn!(error = %e, "Failed to warm relation map cache"),
        }

        Ok(map)
    }

    /// Open the database file and make sure the tables exist.
    fn open_store(config: &RelationMapConfig) -> Result<Database, RelationMapError> {
        let db = Database::create(&config.db_path).map_err(store_err)?;
        let txn = db.begin_write().map_err(store_err)?;
        txn.open_table(TABLE).map_err(store_err)?;
        txn.open_table(META).map_err(store_err)?;
        txn.commit().map_err(store_err)?;
        Ok(db)
    }

    /// Rows written by the last completed rebuild, or None if none completed.
    fn rebuilt_rows(db: &Database) -> Result<Option<u64>, RelationMapError> {
        let txn = db.begin_read().map_err(store_err)?;
        let meta = txn.open_table(META).map_err(store_err)?;
        let v = meta.get(REBUILT_KEY).map_err(store_err)?;
        Ok(v.map(|g| g.value()))
    }

    /// Empty the map and clear the completion marker, in one immediate commit, so a rebuild
    /// starts from nothing and an interrupted one is detected on the next open.
    fn begin_rebuild(&self) -> Result<(), RelationMapError> {
        let txn = self.db.begin_write().map_err(store_err)?;
        {
            let mut meta = txn.open_table(META).map_err(store_err)?;
            meta.remove(REBUILT_KEY).map_err(store_err)?;
        }
        txn.delete_table(TABLE).map_err(store_err)?;
        txn.open_table(TABLE).map_err(store_err)?;
        txn.commit().map_err(store_err)
    }

    /// Record that a rebuild finished, with an immediate (fsynced) commit that also makes
    /// every earlier batch durable. Only after this does a reopen skip the rebuild.
    fn mark_rebuilt(&self, rows: u64) -> Result<(), RelationMapError> {
        let txn = self.db.begin_write().map_err(store_err)?;
        {
            let mut meta = txn.open_table(META).map_err(store_err)?;
            meta.insert(REBUILT_KEY, rows).map_err(store_err)?;
        }
        txn.commit().map_err(store_err)
    }

    /// Load up to `cache_size` entries from the store into the LRU cache.
    fn warm_cache(&self) -> Result<usize, RelationMapError> {
        let txn = self.db.begin_read().map_err(store_err)?;
        let table = txn.open_table(TABLE).map_err(store_err)?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|e| RelationMapError::LockPoisoned(e.to_string()))?;
        let mut count = 0usize;
        for row in table.iter().map_err(store_err)? {
            if count >= self.config.cache_size {
                break;
            }
            match row {
                Ok((k, v)) => {
                    cache.put(Uuid::from_bytes(k.value()), unpack(&v.value()));
                    count += 1;
                }
                Err(e) => warn!(error = %e, "Skipping corrupt row during cache warm"),
            }
        }
        Ok(count)
    }

    /// Whether the store is new and needs a full rebuild from Postgres.
    pub fn needs_rebuild(&self) -> bool {
        self.needs_rebuild
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &RelationMapConfig {
        &self.config
    }

    /// Write one batch in a single transaction with `Eventual` durability. On Linux redb
    /// still fdatasyncs an eventual commit (it differs from immediate only on macOS), so each
    /// committed batch survives a kill.
    fn write_batch(
        &self,
        puts: &[(Uuid, Uuid, Uuid)],
        removes: &[Uuid],
    ) -> Result<(), RelationMapError> {
        let mut txn = self.db.begin_write().map_err(store_err)?;
        txn.set_durability(Durability::Eventual);
        {
            let mut t = txn.open_table(TABLE).map_err(store_err)?;
            for (r, e, s) in puts {
                t.insert(*r.as_bytes(), pack(e, s)).map_err(store_err)?;
            }
            for r in removes {
                t.remove(*r.as_bytes()).map_err(store_err)?;
            }
        }
        txn.commit().map_err(store_err)
    }

    /// Insert a relation mapping. Writes to both LRU cache and store.
    /// Never fails: logs errors and continues.
    pub fn insert(&self, relation_id: Uuid, entity_id: Uuid, space_id: Uuid) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.put(relation_id, (entity_id, space_id));
        }
        if let Err(e) = self.write_batch(&[(relation_id, entity_id, space_id)], &[]) {
            error!(error = %e, relation_id = %relation_id, "Failed to insert into relation map store");
            self.metrics.store_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.metrics.inserts.fetch_add(1, Ordering::Relaxed);
    }

    /// Look up a relation mapping. Checks LRU cache first, then the store.
    /// Returns None if not found in either or on error.
    pub fn lookup(&self, relation_id: &Uuid) -> Option<(Uuid, Uuid)> {
        if let Ok(mut cache) = self.cache.lock() {
            if let Some(&(entity_id, space_id)) = cache.get(relation_id) {
                self.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some((entity_id, space_id));
            }
        }
        self.metrics.cache_misses.fetch_add(1, Ordering::Relaxed);

        let result = self.lookup_store(relation_id);
        match &result {
            Some((entity_id, space_id)) => {
                self.metrics.store_hits.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut cache) = self.cache.lock() {
                    cache.put(*relation_id, (*entity_id, *space_id));
                }
            }
            None => {
                self.metrics.store_misses.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }

    fn lookup_store(&self, relation_id: &Uuid) -> Option<(Uuid, Uuid)> {
        let found = (|| -> Result<Option<(Uuid, Uuid)>, RelationMapError> {
            let txn = self.db.begin_read().map_err(store_err)?;
            let table = txn.open_table(TABLE).map_err(store_err)?;
            let v = table.get(*relation_id.as_bytes()).map_err(store_err)?;
            Ok(v.map(|g| unpack(&g.value())))
        })();
        match found {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "Relation map store lookup failed");
                self.metrics.store_errors.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Remove a relation mapping from both cache and store.
    pub fn remove(&self, relation_id: &Uuid) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.pop(relation_id);
        }
        if let Err(e) = self.write_batch(&[], &[*relation_id]) {
            error!(error = %e, "Failed to remove from relation map store");
            self.metrics.store_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.metrics.removes.fetch_add(1, Ordering::Relaxed);
    }

    /// Make every earlier commit durable. Call on shutdown.
    pub fn checkpoint(&self) {
        let result = self.db.begin_write().map_err(store_err).and_then(|txn| {
            // An immediate (default) commit fsyncs everything before it.
            txn.commit().map_err(store_err)
        });
        if let Err(e) = result {
            warn!(error = %e, "Failed to checkpoint relation map store");
        }
    }

    /// Get current metrics snapshot for logging.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        let cache_len = self.cache.lock().map(|c| c.len()).unwrap_or(0);
        MetricsSnapshot {
            cache_hits: self.metrics.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.metrics.cache_misses.load(Ordering::Relaxed),
            store_hits: self.metrics.store_hits.load(Ordering::Relaxed),
            store_misses: self.metrics.store_misses.load(Ordering::Relaxed),
            store_errors: self.metrics.store_errors.load(Ordering::Relaxed),
            inserts: self.metrics.inserts.load(Ordering::Relaxed),
            removes: self.metrics.removes.load(Ordering::Relaxed),
            cache_len,
            db_size_bytes: self.db_size_bytes(),
        }
    }

    /// Store file size in bytes.
    pub fn db_size_bytes(&self) -> u64 {
        std::fs::metadata(&self.config.db_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Total number of entries in the store.
    pub fn store_count(&self) -> u64 {
        let n = (|| -> Result<u64, RelationMapError> {
            let txn = self.db.begin_read().map_err(store_err)?;
            let table = txn.open_table(TABLE).map_err(store_err)?;
            table.len().map_err(store_err)
        })();
        n.unwrap_or(0)
    }

    /// Rebuild the store from a Postgres connection pool.
    /// Uses cursor-based pagination to avoid loading the entire table into memory.
    ///
    /// Maps each relation to `from_entity_id`, the entity whose document holds it.
    /// `relations.entity_id` is the relation's own reified entity, which has no document.
    ///
    /// Postgres only: no geo-db shadow. This pages every relation in id order with an `id > cursor`
    /// predicate, and geo-db has no read that walks relation statements by id with a resumable
    /// cursor; twinning it waits on that read (RELATION-PAGE-CURSOR).
    pub async fn rebuild_from_postgres(
        &self,
        pool: &sqlx::PgPool,
    ) -> Result<u64, RelationMapError> {
        const BATCH_SIZE: i64 = 10_000;

        info!("Rebuilding relation map from Postgres (batch_size={BATCH_SIZE})...");
        let started = std::time::Instant::now();
        self.begin_rebuild()?;

        let mut total: u64 = 0;
        let mut last_id: Option<Uuid> = None;

        loop {
            let rows: Vec<(Uuid, Uuid, Uuid)> = match last_id {
                None => {
                    sqlx::query_as(
                        "SELECT id, from_entity_id, space_id FROM relations ORDER BY id LIMIT $1",
                    )
                    .bind(BATCH_SIZE)
                    .fetch_all(pool)
                    .await
                }
                Some(cursor) => {
                    sqlx::query_as(
                        "SELECT id, from_entity_id, space_id FROM relations WHERE id > $1 ORDER BY id LIMIT $2",
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

            self.write_batch(&rows, &[])?;

            total += batch_len;

            if total.is_multiple_of(100_000) || batch_len < BATCH_SIZE as u64 {
                info!(
                    progress = total,
                    batch = batch_len,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Relation map rebuild progress"
                );
            }

            // Last batch was smaller than limit — we're done
            if batch_len < BATCH_SIZE as u64 {
                break;
            }
        }

        self.mark_rebuilt(total)?;

        info!(
            total = total,
            elapsed_ms = started.elapsed().as_millis() as u64,
            db_size_mb = self.db_size_bytes() as f64 / 1_048_576.0,
            "Relation map rebuilt from Postgres"
        );

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
            store_hits = m.store_hits,
            store_misses = m.store_misses,
            store_errors = m.store_errors,
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

    fn test_config(dir: &tempfile::TempDir) -> RelationMapConfig {
        RelationMapConfig {
            db_path: dir.path().join("test_relation_map.redb"),
            cache_size: 100,
        }
    }

    #[test]
    fn test_open_creates_fresh_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");
        assert!(
            map.needs_rebuild(),
            "a brand new store has nothing in it yet"
        );
        assert_eq!(map.store_count(), 0);
        map.mark_rebuilt(0).expect("mark");
        drop(map);
        let again = RelationMap::open(test_config(&dir)).expect("reopen");
        assert!(!again.needs_rebuild());
    }

    #[test]
    fn test_interrupted_rebuild_is_redone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rid = Uuid::new_v4();
        {
            let map = RelationMap::open(test_config(&dir)).expect("open");
            assert!(map.needs_rebuild());
            // A rebuild that wrote one batch and was killed before it finished.
            map.begin_rebuild().expect("begin");
            map.write_batch(&[(rid, Uuid::new_v4(), Uuid::new_v4())], &[])
                .expect("batch");
        }
        let map = RelationMap::open(test_config(&dir)).expect("reopen");
        assert!(
            map.needs_rebuild(),
            "a store whose rebuild never finished must be rebuilt, not trusted"
        );

        // The retry starts from an empty map and, once marked, is trusted on reopen.
        map.begin_rebuild().expect("begin again");
        assert_eq!(map.store_count(), 0);
        map.write_batch(&[(rid, Uuid::new_v4(), Uuid::new_v4())], &[])
            .expect("batch");
        map.mark_rebuilt(1).expect("mark");
        drop(map);
        let map = RelationMap::open(test_config(&dir)).expect("reopen after rebuild");
        assert!(!map.needs_rebuild());
        assert_eq!(map.store_count(), 1);
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
        assert_eq!(map.store_count(), 1);
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
        assert_eq!(map.store_count(), 0);
    }

    #[test]
    fn test_store_fallback_after_cache_eviction() {
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

        // rid1 was evicted from cache but should be in the store
        let result = map.lookup(&rid1);
        assert_eq!(result, Some((eid, sid)));

        let m = map.metrics_snapshot();
        assert!(m.store_hits > 0, "Should have hit the store");
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

        // Reopen — cache should be warmed from the store
        {
            let map = RelationMap::open(config).expect("reopen");
            let result = map.lookup(&rid);
            assert_eq!(result, Some((eid, sid)));

            // Should be a cache hit (warmed from the store on open)
            let m = map.metrics_snapshot();
            assert_eq!(m.cache_hits, 1);
            assert_eq!(m.store_hits, 0);
        }
    }

    #[test]
    fn test_integrity_check_failure_recreates_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(&dir);

        // Write garbage to the the store file
        std::fs::write(&config.db_path, b"this is not a store file").expect("write");

        // Open should detect corruption and recreate
        let map = RelationMap::open(config).expect("open after corruption");
        assert!(map.needs_rebuild());
        assert_eq!(map.store_count(), 0);
    }

    #[test]
    fn test_metrics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = RelationMap::open(test_config(&dir)).expect("open");

        let rid = Uuid::new_v4();
        map.insert(rid, Uuid::new_v4(), Uuid::new_v4());
        map.lookup(&rid); // cache hit
        map.lookup(&Uuid::new_v4()); // cache miss + store miss

        let m = map.metrics_snapshot();
        assert_eq!(m.inserts, 1);
        assert_eq!(m.cache_hits, 1);
        assert_eq!(m.cache_misses, 1);
        assert_eq!(m.store_misses, 1);
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
        assert_eq!(map.store_count(), 1); // Still 1, not 2
    }
}
