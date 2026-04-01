//! Storage layer for delivery-worker database operations.

use std::time::Duration;

use hermes_instrumentation::instrument;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;

/// A claimed delivery row ready for webhook delivery.
pub struct PendingDelivery {
    pub delivery_id: Uuid,
    pub outbox_id: Uuid,
    pub webhook_url: String,
    pub webhook_secret: String,
    pub idempotency_key: String,
    pub payload: serde_json::Value,
    pub attempts: i16,
}

/// Storage for delivery-related database operations.
pub struct Storage {
    pool: PgPool,
}

impl Storage {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Connect to the database and create a new Storage instance.
    ///
    /// Pool tuning is configurable via `DB_POOL_MAX_CONNECTIONS` (default 20).
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let max_connections: u32 = std::env::var("DB_POOL_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);

        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Some(Duration::from_secs(600)))
            .max_lifetime(Some(Duration::from_secs(1800)))
            .connect(database_url)
            .await?;
        Ok(Self::new(pool))
    }

    /// Get a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Atomically claim pending deliveries by setting their status to `in_progress`.
    ///
    /// Uses a CTE to SELECT + UPDATE in a single query so the `FOR UPDATE` lock
    /// is released immediately after the claim, rather than being held through
    /// the entire HTTP delivery cycle.
    #[instrument(
        name = "delivery_worker.storage.claim_pending",
        skip(self),
        fields(limit = limit)
    )]
    pub async fn claim_pending(&self, limit: i64) -> Result<Vec<PendingDelivery>, StorageError> {
        let rows = sqlx::query(
            r#"
            WITH claimed AS (
                UPDATE notification_deliveries d
                SET status = 'in_progress',
                    updated_at = now()
                WHERE d.id IN (
                    SELECT d2.id
                    FROM notification_deliveries d2
                    WHERE d2.status = 'pending'
                      AND d2.next_retry_at <= now()
                    ORDER BY d2.next_retry_at ASC
                    LIMIT $1
                    FOR UPDATE OF d2 SKIP LOCKED
                )
                RETURNING d.id, d.outbox_id, d.webhook_id, d.attempts
            )
            SELECT
                c.id as delivery_id,
                c.outbox_id,
                c.attempts,
                w.url as webhook_url,
                w.secret as webhook_secret,
                o.idempotency_key,
                o.payload
            FROM claimed c
            JOIN app_webhooks w ON w.id = c.webhook_id
            JOIN notification_outbox o ON o.id = c.outbox_id
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut deliveries = Vec::with_capacity(rows.len());
        for row in rows {
            deliveries.push(PendingDelivery {
                delivery_id: row.get("delivery_id"),
                outbox_id: row.get("outbox_id"),
                webhook_url: row.get("webhook_url"),
                webhook_secret: row.get("webhook_secret"),
                idempotency_key: row.get("idempotency_key"),
                payload: row.get("payload"),
                attempts: row.get("attempts"),
            });
        }

        Ok(deliveries)
    }

    /// Mark a delivery as successfully delivered.
    #[instrument(name = "delivery_worker.storage.mark_delivered", skip(self))]
    pub async fn mark_delivered(&self, delivery_id: Uuid) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            UPDATE notification_deliveries
            SET status = 'delivered',
                delivered_at = now(),
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record a failed delivery attempt and schedule retry with exponential backoff.
    ///
    /// Resets status back to `pending` so the row re-enters the claimable pool.
    #[instrument(name = "delivery_worker.storage.mark_retry", skip(self, error_msg))]
    pub async fn mark_retry(
        &self,
        delivery_id: Uuid,
        backoff_secs: i64,
        error_msg: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            UPDATE notification_deliveries
            SET status = 'pending',
                attempts = attempts + 1,
                last_error = $2,
                next_retry_at = now() + ($3 || ' seconds')::interval,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .bind(error_msg)
        .bind(backoff_secs.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark a delivery as permanently failed (max retries exceeded).
    #[instrument(name = "delivery_worker.storage.mark_failed", skip(self, error_msg))]
    pub async fn mark_failed(
        &self,
        delivery_id: Uuid,
        error_msg: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            UPDATE notification_deliveries
            SET status = 'failed',
                attempts = attempts + 1,
                last_error = $2,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .bind(error_msg)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Reset stale `in_progress` deliveries back to `pending`.
    ///
    /// Handles the case where a worker crashes mid-delivery, leaving rows
    /// stuck in `in_progress`. Called periodically by the main loop.
    #[instrument(
        name = "delivery_worker.storage.reset_stale_claims",
        skip(self),
        fields(stale_after_secs = stale_after_secs)
    )]
    pub async fn reset_stale_claims(&self, stale_after_secs: i64) -> Result<u64, StorageError> {
        let result = sqlx::query(
            r#"
            UPDATE notification_deliveries
            SET status = 'pending',
                updated_at = now()
            WHERE status = 'in_progress'
              AND updated_at < now() - ($1 || ' seconds')::interval
            "#,
        )
        .bind(stale_after_secs.to_string())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}
