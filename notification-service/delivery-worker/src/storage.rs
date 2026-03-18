//! Storage layer for delivery-worker database operations.

use hermes_instrumentation::instrument;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;

/// A pending delivery row fetched from the database.
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
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let pool = PgPool::connect(database_url).await?;
        Ok(Self::new(pool))
    }

    /// Fetch pending deliveries that are ready for processing.
    ///
    /// Uses `FOR UPDATE SKIP LOCKED` to enable safe concurrent polling
    /// if horizontally scaled in the future.
    #[instrument(name = "delivery_worker.storage.fetch_pending", skip(self), fields(limit = limit))]
    pub async fn fetch_pending(&self, limit: i64) -> Result<Vec<PendingDelivery>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT
                d.id as delivery_id,
                d.outbox_id,
                d.attempts,
                w.url as webhook_url,
                w.secret as webhook_secret,
                o.idempotency_key,
                o.payload
            FROM notification_deliveries d
            JOIN app_webhooks w ON w.id = d.webhook_id
            JOIN notification_outbox o ON o.id = d.outbox_id
            WHERE d.status = 'pending'
              AND d.next_retry_at <= now()
            ORDER BY d.next_retry_at ASC
            LIMIT $1
            FOR UPDATE OF d SKIP LOCKED
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
                delivered_at = now()
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Record a failed delivery attempt with exponential backoff.
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
            SET attempts = attempts + 1,
                last_error = $2,
                next_retry_at = now() + ($3 || ' seconds')::interval
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
                last_error = $2
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .bind(error_msg)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
