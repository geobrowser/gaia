//! Storage layer for notification outbox and delivery fan-out.

use hermes_instrumentation::instrument;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::models::NotificationEvent;

/// Storage for notification-related database operations.
pub struct Storage {
    pool: PgPool,
}

/// An expired proposal found by the rejection poller.
pub struct ExpiredProposal {
    pub id: Uuid,
    pub space_id: Uuid,
    pub proposed_by: Uuid,
    pub end_time: i64,
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

    /// Look up all editors for a given space.
    ///
    /// Returns the `member_space_id` of each editor — this is the editor's account
    /// space UUID and becomes the `user_space_id` in the webhook payload.
    #[instrument(name = "notification_indexer.storage.find_editors_for_space", skip(self))]
    pub async fn find_editors_for_space(
        &self,
        space_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT member_space_id FROM editors WHERE space_id = $1
            "#,
        )
        .bind(space_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(|r| r.get("member_space_id")).collect())
    }

    /// Insert per-editor notifications into the outbox and fan out deliveries.
    ///
    /// For each editor in the space, creates an outbox row with that editor's
    /// `user_space_id` stamped into the payload, then fans out delivery rows
    /// to all registered webhooks. Idempotency keys include the editor ID.
    ///
    /// Returns the number of new outbox rows inserted (0 if all were duplicates).
    #[instrument(name = "notification_indexer.storage.insert_notifications_for_editors", skip(self, event, editors))]
    pub async fn insert_notifications_for_editors(
        &self,
        event: &NotificationEvent,
        editors: &[Uuid],
    ) -> Result<u64, StorageError> {
        let mut inserted_count: u64 = 0;

        for editor_id in editors {
            let mut payload = event.payload.clone();
            payload.user_space_id = Some(editor_id.to_string());
            let idempotency_key = format!("{}:{}", event.idempotency_key, editor_id);
            payload.idempotency_key = Some(idempotency_key.clone());

            let serialized_payload = serde_json::to_value(&payload).map_err(|e| {
                StorageError::Database(sqlx::Error::Protocol(format!(
                    "failed to serialize payload: {}",
                    e
                )))
            })?;

            let mut tx = self.pool.begin().await?;

            // Insert into outbox (ON CONFLICT DO NOTHING for idempotency)
            let result = sqlx::query(
                r#"
                INSERT INTO notification_outbox (idempotency_key, event_type, payload)
                VALUES ($1, $2, $3)
                ON CONFLICT (idempotency_key) DO NOTHING
                RETURNING id
                "#,
            )
            .bind(&idempotency_key)
            .bind(event.event_type.as_str())
            .bind(&serialized_payload)
            .fetch_optional(&mut *tx)
            .await?;

            let outbox_id: Uuid = match result {
                Some(row) => row.get("id"),
                None => {
                    // Duplicate — already processed
                    tx.rollback().await?;
                    continue;
                }
            };

            // Fan out: create a delivery row for every registered webhook
            sqlx::query(
                r#"
                INSERT INTO notification_deliveries (outbox_id, webhook_id)
                SELECT $1, id FROM app_webhooks
                "#,
            )
            .bind(outbox_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            inserted_count += 1;
        }

        Ok(inserted_count)
    }

    /// Find proposals that have expired (end_time < now) without being executed,
    /// and for which we haven't yet sent a rejection notification.
    #[instrument(name = "notification_indexer.storage.find_expired_proposals", skip(self))]
    pub async fn find_expired_proposals(&self) -> Result<Vec<ExpiredProposal>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT p.id, p.space_id, p.proposed_by, p.end_time
            FROM proposals p
            LEFT JOIN notification_outbox o
              ON o.event_type = 'proposal_rejected'
             AND (o.payload->>'proposal_id')::uuid = p.id
            WHERE p.end_time < EXTRACT(EPOCH FROM now())
              AND p.executed_at IS NULL
              AND o.id IS NULL
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut expired = Vec::with_capacity(rows.len());
        for row in rows {
            expired.push(ExpiredProposal {
                id: row.get("id"),
                space_id: row.get("space_id"),
                proposed_by: row.get("proposed_by"),
                end_time: row.get::<i64, _>("end_time"),
            });
        }

        Ok(expired)
    }
}
