//! Storage layer for delivery-worker database operations.

use std::time::Duration;

use hermes_instrumentation::instrument;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::webhooks_file::{WebhookDirectory, WebhookFile};

/// A claimed delivery row ready for webhook delivery.
pub struct PendingDelivery {
    pub delivery_id: Uuid,
    pub outbox_id: Uuid,
    pub webhook_id: Uuid,
    pub webhook_url: String,
    pub webhook_secret: String,
    pub idempotency_key: String,
    pub payload: serde_json::Value,
    pub attempts: i16,
}

/// Storage for delivery-related database operations.
pub struct Storage {
    pool: PgPool,
    /// `Some` when `APP_WEBHOOKS_FILE` is set: url + secret come from the
    /// mounted file instead of the `app_webhooks` table.
    webhooks_file: Option<WebhookFile>,
}

/// Outcome of resolving a claimed delivery's webhook in file mode.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Found,
    /// The webhook id is not in the file (the file-mode analogue of a deleted
    /// `app_webhooks` row).
    Unknown,
}

/// Error recorded on a delivery whose webhook id is not in the file. Never
/// contains the url or secret.
pub fn unknown_webhook_error(webhook_id: Uuid) -> String {
    format!("webhook {webhook_id} not found in app webhooks file")
}

/// Look up a webhook in the parsed file.
pub fn resolve(dir: &WebhookDirectory, webhook_id: &Uuid) -> Resolution {
    if dir.get(webhook_id).is_some() {
        Resolution::Found
    } else {
        Resolution::Unknown
    }
}

impl Storage {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            webhooks_file: WebhookFile::from_env(),
        }
    }

    /// Override the webhook source (used by tests).
    pub fn with_webhooks_file(mut self, file: Option<WebhookFile>) -> Self {
        self.webhooks_file = file;
        self
    }

    /// Whether url + secret are read from the mounted file.
    pub fn uses_webhooks_file(&self) -> bool {
        self.webhooks_file.is_some()
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
        if let Some(file) = &self.webhooks_file {
            return self.claim_pending_from_file(file, limit).await;
        }
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
                c.webhook_id,
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
                webhook_id: row.get("webhook_id"),
                webhook_url: row.get("webhook_url"),
                webhook_secret: row.get("webhook_secret"),
                idempotency_key: row.get("idempotency_key"),
                payload: row.get("payload"),
                attempts: row.get("attempts"),
            });
        }

        Ok(deliveries)
    }

    /// File-mode claim: same claim as above without the `app_webhooks` JOIN;
    /// url + secret are resolved by `webhook_id` from the mounted file. A claimed
    /// delivery whose id is not in the file is marked failed (like a delivery
    /// whose webhook row was deleted) and logged without any webhook values.
    async fn claim_pending_from_file(
        &self,
        file: &WebhookFile,
        limit: i64,
    ) -> Result<Vec<PendingDelivery>, StorageError> {
        // Read the file before claiming so a missing/invalid file leaves rows
        // `pending` rather than stranding them `in_progress`.
        let dir = match file.load().await {
            Ok(d) => d,
            Err(e) => {
                hermes_instrumentation::error!(error = %e, "app webhooks file unavailable; not claiming");
                return Ok(Vec::new());
            }
        };
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
                c.webhook_id,
                c.attempts,
                o.idempotency_key,
                o.payload
            FROM claimed c
            JOIN notification_outbox o ON o.id = c.outbox_id
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut deliveries = Vec::with_capacity(rows.len());
        for row in rows {
            let delivery_id: Uuid = row.get("delivery_id");
            let webhook_id: Uuid = row.get("webhook_id");
            let Some(target) = dir.get(&webhook_id) else {
                let msg = unknown_webhook_error(webhook_id);
                hermes_instrumentation::error!(
                    delivery_id = %delivery_id,
                    webhook_id = %webhook_id,
                    "webhook id not in app webhooks file; marking delivery failed"
                );
                self.mark_failed(delivery_id, &msg).await?;
                continue;
            };
            deliveries.push(PendingDelivery {
                delivery_id,
                outbox_id: row.get("outbox_id"),
                webhook_id,
                webhook_url: target.url.clone(),
                webhook_secret: target.secret.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_and_unknown() {
        let id = Uuid::from_u128(1);
        let json = format!(r#"[{{"id":"{id}","url":"https://x.example","secret":"topsecret"}}]"#);
        let dir = WebhookDirectory::parse(json.as_bytes()).unwrap();
        assert_eq!(resolve(&dir, &id), Resolution::Found);
        assert_eq!(resolve(&dir, &Uuid::from_u128(2)), Resolution::Unknown);
    }

    #[test]
    fn unknown_error_has_no_url_or_secret() {
        let msg = unknown_webhook_error(Uuid::from_u128(2));
        assert!(msg.contains("00000000-0000-0000-0000-000000000002"));
        assert!(!msg.contains("topsecret") && !msg.contains("https://"));
    }

    #[tokio::test]
    async fn unset_env_keeps_postgres_path() {
        std::env::remove_var(crate::webhooks_file::ENV_VAR);
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://u:p@localhost/db")
            .unwrap();
        assert!(!Storage::new(pool).uses_webhooks_file());
    }
}
