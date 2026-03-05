//! Storage layer for topology distance data.

use hermes_instrumentation::instrument;
use sqlx::PgPool;
use uuid::Uuid;

use crate::consumer::{ChangeType, TopologyChange};
use crate::error::StorageError;

/// Storage for topology distance database operations.
pub struct Storage {
    pool: PgPool,
}

impl Storage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        let mut tx = self.pool.begin().await?;

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
                        upsert_distances.push(distance as i32);
                    }
                }
                ChangeType::Removed => {
                    delete_ids.push(change.space_id);
                }
            }
        }

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
            .bind(&upsert_ids)
            .bind(&upsert_distances)
            .execute(&mut *tx)
            .await?;
        }

        // Bulk DELETE for Removed changes
        if !delete_ids.is_empty() {
            sqlx::query("DELETE FROM scoring_topology_distances WHERE space_id = ANY($1)")
                .bind(&delete_ids)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;

        Ok(())
    }
}
