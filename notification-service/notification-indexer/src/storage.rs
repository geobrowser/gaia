//! Storage layer for notification outbox and delivery fan-out.

use std::time::Duration;

use hermes_instrumentation::instrument;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::models::NotificationEvent;

// Well-known system IDs from the knowledge graph.
// Sourced from geo-sdk (src/core/ids/system.ts) and curator-app (packages/curator-utils/src/ids.ts).
// These are being upstreamed to the grc-20 Rust crate — once available, import from there instead.

/// The "Types" relation type ID — used for entity type membership (e.g. entity → Bounty type).
const TYPE_RELATION_TYPE_ID: &str = "8f151ba4-de20-4e3c-9cb4-99ddf96f48f1";
/// The "Space" type entity ID — used to identify space entities via Types relations.
const SPACE_TYPE_ID: &str = "362c1dbd-dc64-44bb-a3c4-652f38a642d7";
/// The "Bounty" type entity ID — used to identify bounty entities via Types relations.
const BOUNTY_TYPE_ID: &str = "808af0ba-d588-4e33-91f0-9dd4b25e18be";
/// The "Name" property ID — used to look up entity display names.
const NAME_PROPERTY_ID: &str = "a126ca53-0c8e-48d5-b888-82c734c38935";

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

    /// Look up all editors for a given space.
    ///
    /// Returns the `member_space_id` of each editor — this is the editor's account
    /// space UUID and becomes the `user_space_id` in the webhook payload.
    #[instrument(
        name = "notification_indexer.storage.find_editors_for_space",
        skip(self)
    )]
    pub async fn find_editors_for_space(&self, space_id: Uuid) -> Result<Vec<Uuid>, StorageError> {
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

    /// Insert per-user notifications into the outbox and fan out deliveries.
    ///
    /// For each recipient user, creates an outbox row with that user's
    /// `user_space_id` stamped into the payload, then fans out delivery rows
    /// to all registered webhooks. Idempotency keys include the user ID.
    ///
    /// Recipients are the relevant *superset* for the event (e.g. a space's
    /// editors, plus the proposer or prior voters for targeted governance
    /// events). The caller is responsible for de-duplicating the slice; the
    /// `ON CONFLICT (idempotency_key) DO NOTHING` clause also makes duplicate
    /// recipients a no-op. Filtering to a specific audience is done app-side.
    ///
    /// Returns the number of new outbox rows inserted (0 if all were duplicates).
    #[instrument(
        name = "notification_indexer.storage.insert_notifications_for_users",
        skip(self, event, users)
    )]
    pub async fn insert_notifications_for_users(
        &self,
        event: &NotificationEvent,
        users: &[Uuid],
    ) -> Result<u64, StorageError> {
        let mut inserted_count: u64 = 0;

        // Serialize the payload once before the recipient loop. Per-user fields
        // (user_space_id, idempotency_key) are stamped into the Value clone,
        // avoiding N struct clones + N serializations.
        let base_value = serde_json::to_value(&event.payload).map_err(|e| {
            StorageError::Database(sqlx::Error::Protocol(format!(
                "failed to serialize payload: {}",
                e
            )))
        })?;

        // Single transaction for all recipients — either all fan-out rows are
        // committed or none are. On error the transaction is dropped (implicit
        // rollback), and the Kafka offset is not committed so the message
        // will be reprocessed. The ON CONFLICT DO NOTHING clause on the outbox
        // insert ensures safe reprocessing of already-committed recipients.
        let mut tx = self.pool.begin().await?;

        for user_id in users {
            // Raw key for debugging: e.g. "12345:0:proposal_created:user-uuid"
            let raw_key = format!("{}:{}", event.idempotency_key, user_id);
            // SHA-256 hash for the DB UNIQUE constraint (fixed-length, collision-resistant)
            let db_key = hex::encode(Sha256::digest(raw_key.as_bytes()));

            // Clone the pre-serialized Value and stamp per-user fields.
            // The payload sends the raw string so app servers can debug/log it;
            // the DB stores the hash for indexing.
            let mut serialized_payload = base_value.clone();
            if let serde_json::Value::Object(ref mut map) = serialized_payload {
                map.insert(
                    "user_space_id".to_string(),
                    serde_json::Value::String(user_id.to_string()),
                );
                map.insert(
                    "idempotency_key".to_string(),
                    serde_json::Value::String(raw_key.clone()),
                );
            }

            // Insert into outbox (ON CONFLICT DO NOTHING for idempotency)
            let result = sqlx::query(
                r#"
                INSERT INTO notification_outbox (idempotency_key, event_type, payload)
                VALUES ($1, $2, $3)
                ON CONFLICT (idempotency_key) DO NOTHING
                RETURNING id
                "#,
            )
            .bind(&db_key)
            .bind(event.event_type.as_str())
            .bind(&serialized_payload)
            .fetch_optional(&mut *tx)
            .await?;

            let outbox_id: Uuid = match result {
                Some(row) => row.get("id"),
                None => {
                    // Duplicate — already processed, skip this recipient
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

            inserted_count += 1;
        }

        tx.commit().await?;

        Ok(inserted_count)
    }

    // -----------------------------------------------------------------------
    // Bounty entity/space resolution
    // -----------------------------------------------------------------------

    /// Resolve a user entity_id to their personal space UUID.
    ///
    /// Uses the "front page entity" pattern: finds a personal space
    /// that has a Types relation pointing from the entity to SPACE_TYPE.
    #[instrument(name = "notification_indexer.storage.lookup_entity_space", skip(self))]
    pub async fn lookup_entity_space(&self, entity_id: Uuid) -> Result<Option<Uuid>, StorageError> {
        let type_rel_id =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("valid TYPE_RELATION_TYPE_ID");
        let space_type_id = Uuid::parse_str(SPACE_TYPE_ID).expect("valid SPACE_TYPE_ID");
        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT r.space_id
            FROM relations r
            JOIN spaces s ON s.id = r.space_id
            WHERE r.from_entity_id = $1
              AND r.type_id = $2
              AND r.to_entity_id = $3
            LIMIT 1
            "#,
        )
        .bind(entity_id)
        .bind(type_rel_id)
        .bind(space_type_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Look up which space owns a bounty entity (for interest notifications).
    ///
    /// Finds the space via the bounty's Types relation pointing to BOUNTY_TYPE.
    /// Requires both type_id (TYPE_RELATION_TYPE_ID) and to_entity_id (BOUNTY_TYPE)
    /// to avoid matching unrelated Types relations from the same entity.
    #[instrument(name = "notification_indexer.storage.lookup_bounty_space", skip(self))]
    pub async fn lookup_bounty_space(
        &self,
        bounty_entity_id: Uuid,
    ) -> Result<Option<Uuid>, StorageError> {
        let type_rel_id =
            Uuid::parse_str(TYPE_RELATION_TYPE_ID).expect("valid TYPE_RELATION_TYPE_ID");
        let bounty_type_id = Uuid::parse_str(BOUNTY_TYPE_ID).expect("valid BOUNTY_TYPE_ID");
        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT space_id
            FROM relations
            WHERE from_entity_id = $1
              AND type_id = $2
              AND to_entity_id = $3
            LIMIT 1
            "#,
        )
        .bind(bounty_entity_id)
        .bind(type_rel_id)
        .bind(bounty_type_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Insert a notification for a single user (curator) into the outbox.
    ///
    /// Used for bounty_allocated and bounty_payout where there's one recipient.
    /// Delegates to `insert_notifications_for_users` with a single-element slice.
    #[instrument(
        name = "notification_indexer.storage.insert_notification_for_user",
        skip(self, event)
    )]
    pub async fn insert_notification_for_user(
        &self,
        event: &NotificationEvent,
        user_space_id: Uuid,
    ) -> Result<u64, StorageError> {
        self.insert_notifications_for_users(event, &[user_space_id])
            .await
    }

    /// Resolve the proposer (`proposed_by`) of a proposal as a recipient
    /// `user_space_id`.
    ///
    /// `proposals.proposed_by` is the proposer's personal-space UUID (the proto
    /// `proposer_id` is "the space creating the proposal"), so it is directly
    /// usable as a notification recipient — no entity→space resolution needed.
    /// Used to deliver "your proposal was voted on / approved / rejected".
    #[instrument(name = "notification_indexer.storage.find_proposer_for_proposal", skip(self))]
    pub async fn find_proposer_for_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<Option<Uuid>, StorageError> {
        let result = sqlx::query_scalar::<_, Uuid>(
            "SELECT proposed_by FROM proposals WHERE id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
    }

    /// Resolve the distinct voters of a proposal as recipient `user_space_id`s.
    ///
    /// `proposal_votes.voter_id` is the voter's personal-space UUID (the proto
    /// `voter_id` is "the space casting the vote"), so it is directly usable as
    /// a notification recipient. Used to deliver "a new version of a proposal
    /// you voted on was submitted" to prior voters.
    #[instrument(name = "notification_indexer.storage.find_voters_for_proposal", skip(self))]
    pub async fn find_voters_for_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let rows = sqlx::query(
            "SELECT DISTINCT voter_id FROM proposal_votes WHERE proposal_id = $1",
        )
        .bind(proposal_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get("voter_id")).collect())
    }

    /// Find proposals that have expired (end_time < now) without being executed,
    /// and for which we haven't yet sent a rejection notification.
    ///
    /// Results are limited to `limit` rows to prevent unbounded memory usage.
    /// Callers should loop until a batch returns fewer than `limit` rows.
    #[instrument(
        name = "notification_indexer.storage.find_expired_proposals",
        skip(self),
        fields(limit = limit)
    )]
    pub async fn find_expired_proposals(
        &self,
        limit: i64,
    ) -> Result<Vec<ExpiredProposal>, StorageError> {
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
            LIMIT $1
            "#,
        )
        .bind(limit)
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

    // -----------------------------------------------------------------------
    // Enrichment lookups (best-effort — return None on failure)
    // -----------------------------------------------------------------------

    /// Look up a human-readable name for an entity from the KG values table.
    ///
    /// Returns `None` if the entity has no name or the query fails.
    pub async fn lookup_entity_name(&self, entity_id: Uuid, space_id: Uuid) -> Option<String> {
        let name_prop_id = Uuid::parse_str(NAME_PROPERTY_ID).expect("valid NAME_PROPERTY_ID");
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT v.text
            FROM "values" v
            WHERE v.entity_id = $1
              AND v.property_id = $3
              AND v.space_id = $2
              AND v.text IS NOT NULL
            LIMIT 1
            "#,
        )
        .bind(entity_id)
        .bind(space_id)
        .bind(name_prop_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Look up the human-readable name for a proposal from the proposals table.
    pub async fn lookup_proposal_name(&self, proposal_id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM proposals WHERE id = $1 AND name IS NOT NULL",
        )
        .bind(proposal_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Look up current vote tallies for a proposal.
    pub async fn lookup_vote_tallies(&self, proposal_id: Uuid) -> Option<(i64, i64, i64)> {
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT yes_count, no_count, abstain_count FROM proposals WHERE id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Get the latest block number written by the kg-indexer.
    ///
    /// Used by the block delay logic to wait for the kg-indexer to catch up
    /// before processing notifications (so names/metadata are populated).
    pub async fn lookup_latest_block(&self) -> Option<u64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT MAX(created_at_block::bigint) FROM proposals WHERE created_at_block != '0'",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .map(|b| b as u64)
    }
}
