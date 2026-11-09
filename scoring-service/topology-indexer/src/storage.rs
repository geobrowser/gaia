//! Storage layer for topology distance data.

use std::sync::Arc;

use geodb_cursor::derived::DerivedMirror;
use hermes_instrumentation::instrument;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::consumer::{ChangeType, TopologyChange};
use crate::error::StorageError;
use crate::topology_mirror;

/// Storage for topology distance database operations.
#[derive(Clone)]
pub struct Storage {
    pool: PgPool,
    /// Best-effort copy of the distances into geo-db; `None` unless `GEODB_DERIVED_MIRROR` is on.
    derived: Option<Arc<DerivedMirror>>,
    /// `TOPOLOGY_INDEXER_PG_WRITES`, default on. Off: `apply_changes` issues no Postgres statement
    /// and counts the skip instead (cutover switch, plan 10.22). The geo-db mirror still runs.
    pg_writes: bool,
}

/// Parse `TOPOLOGY_INDEXER_PG_WRITES`. Unset, empty or anything but an explicit off value means
/// on, so a typo never silently stops the writes.
pub fn pg_writes_enabled_from(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("pool", &self.pool)
            .field("derived_mirror", &self.derived.is_some())
            .finish()
    }
}

impl Storage {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            derived: DerivedMirror::from_env().map(Arc::new),
            pg_writes: pg_writes_enabled_from(
                std::env::var("TOPOLOGY_INDEXER_PG_WRITES").ok().as_deref(),
            ),
        }
    }

    /// Override the Postgres-writes switch (the env var sets the default).
    pub fn with_pg_writes(mut self, on: bool) -> Self {
        self.pg_writes = on;
        self
    }

    /// Copies the whole table into geo-db once, so a put the mirror dropped is repaired. A read
    /// error skips the copy, never the indexer. Does nothing while the flag is off.
    pub async fn mirror_snapshot(&self) {
        let Some(mirror) = &self.derived else {
            return;
        };
        match sqlx::query_as::<_, (Uuid, i32)>(topology_mirror::SELECT_SQL)
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => {
                topology_mirror::snapshot(mirror, &rows).await;
                info!(rows = rows.len(), "topology distances mirrored to geo-db");
            }
            Err(e) => warn!(error = %e, "topology mirror snapshot skipped: read failed"),
        }
    }

    /// Connect to the database and create a new Storage instance.
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self::new(pool))
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Apply a batch of topology changes within a single transaction.
    ///
    /// - Added/Moved: UPSERT into scoring_topology_distances with the new distance
    /// - Removed: DELETE from scoring_topology_distances
    /// - Root: UPSERT with distance=0
    #[instrument(name = "topology_indexer.storage.apply_changes", skip(self, root_id, changes), fields(change_count = changes.len()))]
    pub async fn apply_changes(
        &self,
        root_id: Uuid,
        changes: &[TopologyChange],
    ) -> Result<(), StorageError> {
        // Separate changes into upserts and deletes
        let mut upsert_ids: Vec<Uuid> = Vec::new();
        let mut upsert_distances: Vec<i32> = Vec::new();
        let mut delete_ids: Vec<Uuid> = Vec::new();

        // Always upsert root with distance=0
        upsert_ids.push(root_id);
        upsert_distances.push(0);

        for change in changes {
            match change.change_type {
                ChangeType::Added | ChangeType::Moved => {
                    if let Some(distance) = change.distance {
                        upsert_ids.push(change.space_id);
                        let distance_i32 = i32::try_from(distance).map_err(|_| {
                            StorageError::Conversion(format!(
                                "distance {} exceeds i32 range for space {}",
                                distance, change.space_id
                            ))
                        })?;
                        upsert_distances.push(distance_i32);
                    }
                }
                ChangeType::Removed => {
                    // Never delete the root row — it was upserted above with distance=0
                    if change.space_id == root_id {
                        continue;
                    }
                    delete_ids.push(change.space_id);
                }
            }
        }

        if !self.pg_writes {
            crate::metrics::pg_write_skipped(crate::metrics::PgWriteSite::ApplyChanges);
        } else {
            self.write_postgres(&upsert_ids, &upsert_distances, &delete_ids)
                .await?;
        }

        // After the commit; the mirror is best-effort and never fails the batch.
        if let Some(mirror) = &self.derived {
            let upserts: Vec<(Uuid, i32)> = upsert_ids
                .iter()
                .copied()
                .zip(upsert_distances.iter().copied())
                .collect();
            topology_mirror::mirror(mirror, &upserts, &delete_ids).await;
        }

        Ok(())
    }

    /// The two statements of a batch, in one transaction.
    async fn write_postgres(
        &self,
        upsert_ids: &[Uuid],
        upsert_distances: &[i32],
        delete_ids: &[Uuid],
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;

        // Bulk UPSERT for Added/Moved changes
        if !upsert_ids.is_empty() {
            sqlx::query(
                r#"
                INSERT INTO scoring_topology_distances (space_id, distance, updated_at)
                SELECT space_id, distance, now()
                FROM UNNEST($1::uuid[], $2::integer[])
                    AS t(space_id, distance)
                ON CONFLICT (space_id)
                DO UPDATE SET
                    distance = EXCLUDED.distance,
                    updated_at = EXCLUDED.updated_at
                "#,
            )
            .bind(upsert_ids)
            .bind(upsert_distances)
            .execute(&mut *tx)
            .await?;
        }

        // Bulk DELETE for Removed changes
        if !delete_ids.is_empty() {
            sqlx::query("DELETE FROM scoring_topology_distances WHERE space_id = ANY($1)")
                .bind(delete_ids)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
