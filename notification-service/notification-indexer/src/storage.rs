//! Storage layer for notification outbox and delivery fan-out.

use std::time::Duration;

use hermes_instrumentation::instrument;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::ids;
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

/// A `votes_count` row for an entity, returned by the vote poller's keyset scan.
pub struct EntityVoteCount {
    /// `votes_count.id` (serial) — the keyset tiebreaker and cursor component.
    pub cursor_id: i64,
    /// `object_id` — the entity that was voted on (recipient = its creator).
    pub entity_id: Uuid,
    /// The space the votes were counted in.
    pub space_id: Uuid,
    pub upvotes: i64,
    pub downvotes: i64,
    /// `updated_at` — the keyset primary key and cursor high-water mark.
    pub updated_at: DateTime<Utc>,
    /// Whether this (entity, space) was already notified at the queried threshold.
    /// The poller skips notifying these but still advances the cursor over them.
    pub already_notified: bool,
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
        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT r.space_id
            FROM relations r
            JOIN spaces s ON s.id = r.space_id
            WHERE r.from_entity_id = $1
              AND r.type_id = $2
              AND r.to_entity_id = $3
            ORDER BY r.space_id
            LIMIT 1
            "#,
        )
        .bind(entity_id)
        .bind(ids::types_relation_type())
        .bind(ids::space_type())
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
        let result = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT space_id
            FROM relations
            WHERE from_entity_id = $1
              AND type_id = $2
              AND to_entity_id = $3
            ORDER BY space_id
            LIMIT 1
            "#,
        )
        .bind(bounty_entity_id)
        .bind(ids::types_relation_type())
        .bind(ids::bounty_type())
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
    #[instrument(
        name = "notification_indexer.storage.find_proposer_for_proposal",
        skip(self)
    )]
    pub async fn find_proposer_for_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<Option<Uuid>, StorageError> {
        let result =
            sqlx::query_scalar::<_, Uuid>("SELECT proposed_by FROM proposals WHERE id = $1")
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
    #[instrument(
        name = "notification_indexer.storage.find_voters_for_proposal",
        skip(self)
    )]
    pub async fn find_voters_for_proposal(
        &self,
        proposal_id: Uuid,
    ) -> Result<Vec<Uuid>, StorageError> {
        let rows =
            sqlx::query("SELECT DISTINCT voter_id FROM proposal_votes WHERE proposal_id = $1")
                .bind(proposal_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.iter().map(|r| r.get("voter_id")).collect())
    }

    /// Resolve a proposal's proposer (`proposed_by`) and owning `space_id`.
    ///
    /// Returns `None` if no proposal with that id exists — which is how the
    /// proposal-comment path distinguishes "comment replies to a proposal" from
    /// "comment replies to some other entity" (the latter is a general comment,
    /// handled in a later phase). The proposer is the recipient; the space
    /// scopes the "commenter must be a member/editor" filter.
    #[instrument(
        name = "notification_indexer.storage.find_proposal_proposer_and_space",
        skip(self)
    )]
    pub async fn find_proposal_proposer_and_space(
        &self,
        proposal_id: Uuid,
    ) -> Result<Option<(Uuid, Uuid)>, StorageError> {
        let row = sqlx::query("SELECT proposed_by, space_id FROM proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| (r.get("proposed_by"), r.get("space_id"))))
    }

    /// Whether `user_space_id` is a member or editor of `space_id`.
    ///
    /// Gates proposal-comment notifications on the commenter being a member or
    /// editor of the proposal's space (per the product requirement that proposal
    /// comments come from a space member/editor).
    #[instrument(name = "notification_indexer.storage.is_member_or_editor", skip(self))]
    pub async fn is_member_or_editor(
        &self,
        space_id: Uuid,
        user_space_id: Uuid,
    ) -> Result<bool, StorageError> {
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM editors WHERE space_id = $1 AND member_space_id = $2
                UNION ALL
                SELECT 1 FROM members WHERE space_id = $1 AND member_space_id = $2
            )
            "#,
        )
        .bind(space_id)
        .bind(user_space_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    // -----------------------------------------------------------------------
    // Comment-thread resolution (Phase 2b)
    // -----------------------------------------------------------------------

    /// Walk `Reply to` relations upward from `parent` to the thread root.
    ///
    /// A comment replies to its parent; that parent may itself be a comment that
    /// replies to another, and so on. The root is the first ancestor with no
    /// outgoing `Reply to` relation (the actual "thing being commented on").
    /// Bounded to a fixed depth as a cycle/abuse guard.
    #[instrument(name = "notification_indexer.storage.resolve_thread_root", skip(self))]
    pub async fn resolve_thread_root(&self, parent: Uuid) -> Result<Uuid, StorageError> {
        let reply_to = ids::reply_to_property();
        let mut current = parent;
        for _ in 0..64 {
            // Deterministic parent choice: an entity could carry multiple `Reply to`
            // relations; ORDER BY keeps the resolved thread root stable across runs.
            let next: Option<Uuid> = sqlx::query_scalar::<_, Uuid>(
                "SELECT to_entity_id FROM relations WHERE type_id = $1 AND from_entity_id = $2 \
                 ORDER BY to_entity_id LIMIT 1",
            )
            .bind(reply_to)
            .bind(current)
            .fetch_optional(&self.pool)
            .await?;
            match next {
                Some(n) if n != current => current = n,
                _ => break,
            }
        }
        Ok(current)
    }

    /// All distinct participant spaces in the thread rooted at `root`.
    ///
    /// Participants are the authors of every comment in the thread — derived
    /// from the `space_id` of each `Reply to` relation in the subtree (a
    /// relation's `space_id` is the space it was published from, i.e. the
    /// comment author's personal space). `UNION` (not `UNION ALL`) makes the
    /// recursion cycle-safe.
    #[instrument(
        name = "notification_indexer.storage.find_thread_participants",
        skip(self)
    )]
    pub async fn find_thread_participants(&self, root: Uuid) -> Result<Vec<Uuid>, StorageError> {
        let reply_to = ids::reply_to_property();
        let rows = sqlx::query(
            r#"
            WITH RECURSIVE thread AS (
                SELECT from_entity_id AS comment_id, space_id
                FROM relations
                WHERE type_id = $1 AND to_entity_id = $2
                UNION
                SELECT r.from_entity_id, r.space_id
                FROM relations r
                JOIN thread t ON r.to_entity_id = t.comment_id
                WHERE r.type_id = $1
            )
            SELECT DISTINCT space_id FROM thread
            "#,
        )
        .bind(reply_to)
        .bind(root)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get("space_id")).collect())
    }

    /// Best-effort "home" space of an entity — the `space_id` of its `Types`
    /// relation (where it was created). For entities created in a personal space
    /// this equals the creator. Used as the root-creator recipient for
    /// non-proposal comment threads (proposals resolve their creator exactly via
    /// `find_proposal_proposer_and_space`).
    ///
    /// An entity may have several `Types` relations (multi-typed, or re-typed in
    /// a later edit). `ORDER BY space_id` makes the choice deterministic across
    /// runs — still best-effort, but stable, so recipients and the payload
    /// `space_id` don't flap between replays.
    #[instrument(
        name = "notification_indexer.storage.find_entity_home_space",
        skip(self)
    )]
    pub async fn find_entity_home_space(
        &self,
        entity_id: Uuid,
    ) -> Result<Option<Uuid>, StorageError> {
        let result = sqlx::query_scalar::<_, Uuid>(
            "SELECT space_id FROM relations WHERE from_entity_id = $1 AND type_id = $2 \
             ORDER BY space_id LIMIT 1",
        )
        .bind(entity_id)
        .bind(ids::types_relation_type())
        .fetch_optional(&self.pool)
        .await?;
        Ok(result)
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
            ORDER BY p.end_time ASC, p.id
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
        .bind(ids::name_property())
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    /// Look up a space's display name (e.g. "Wonderland").
    ///
    /// The bare `space_id` entity only carries the auto-generated placeholder
    /// ("Space <uuid>"). The real name lives on the space's *page entity* — the
    /// entity inside the space with a Types relation to SPACE_TYPE (the same
    /// "front page entity" pattern as `lookup_entity_space`, and what the API's
    /// `spaces_page` function resolves). We read that entity's Name value.
    ///
    /// Name values are not space-scoped (matching the API's `entities_name`),
    /// so we match on `entity_id` only. Returns `None` if the space has no page
    /// entity or it has no name.
    ///
    /// The schema enforces no uniqueness on the matched relation/value keys (an
    /// entity can carry multiple Name values, e.g. per `language`), so we apply
    /// a deterministic tie-break before `LIMIT 1` — same convention as the API's
    /// `batchGetEntityNames`: pick the lowest page-entity id, prefer the Name
    /// value defined in this space, then the lowest value id.
    pub async fn lookup_space_name(&self, space_id: Uuid) -> Option<String> {
        sqlx::query_scalar::<_, String>(
            r#"
            SELECT v.text
            FROM relations r
            JOIN "values" v ON v.entity_id = r.from_entity_id
            WHERE r.space_id = $1
              AND r.type_id = $2
              AND r.to_entity_id = $3
              AND v.property_id = $4
              AND v.text IS NOT NULL
            ORDER BY r.from_entity_id, (v.space_id = $1) DESC, v.id
            LIMIT 1
            "#,
        )
        .bind(space_id)
        .bind(ids::types_relation_type())
        .bind(ids::space_type())
        .bind(ids::name_property())
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

    // -----------------------------------------------------------------------
    // Entity-vote threshold poller
    // -----------------------------------------------------------------------

    /// Entity `votes_count` rows changed since the keyset cursor.
    ///
    /// Returns rows with `object_type = 0` (entities) ordered by `(updated_at, id)`
    /// strictly greater than the cursor, capped at `limit`. The upvote-threshold
    /// check is done by the caller so the cursor can advance over *every* scanned
    /// row (not just those over the threshold), preventing skips. Backed by the
    /// partial index `idx_votes_count_updated_at (updated_at, id) WHERE object_type = 0`.
    ///
    /// Each row carries `already_notified`: whether an `entity_votes_threshold`
    /// notification already exists for this `(entity, space)` at `threshold` (via
    /// the `idx_outbox_entity_votes_threshold` expression index). The poller skips
    /// those — avoiding a redundant creator lookup + insert — while still advancing
    /// the cursor past them.
    #[instrument(
        name = "notification_indexer.storage.find_entity_vote_counts_since",
        skip(self)
    )]
    pub async fn find_entity_vote_counts_since(
        &self,
        cursor_updated_at: DateTime<Utc>,
        cursor_id: i64,
        threshold: i64,
        limit: i64,
    ) -> Result<Vec<EntityVoteCount>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT vc.id::bigint AS id, vc.object_id, vc.space_id, vc.upvotes, vc.downvotes,
                   vc.updated_at,
                   EXISTS (
                       SELECT 1 FROM notification_outbox o
                       WHERE o.event_type = 'entity_votes_threshold'
                         AND (o.payload->>'entity_id')::uuid = vc.object_id
                         AND (o.payload->>'vote_space_id')::uuid = vc.space_id
                         AND (o.payload->>'threshold')::bigint = $3
                   ) AS already_notified
            FROM votes_count vc
            WHERE vc.object_type = 0
              -- $2::int matches vc.id (int4) so the (updated_at, id) keyset index is
              -- used as-is; cursor_id is always within int4 range (from votes_count.id).
              AND (vc.updated_at, vc.id) > ($1, $2::int)
            ORDER BY vc.updated_at, vc.id
            LIMIT $4
            "#,
        )
        .bind(cursor_updated_at)
        .bind(cursor_id)
        .bind(threshold)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| EntityVoteCount {
                cursor_id: r.get("id"),
                entity_id: r.get("object_id"),
                space_id: r.get("space_id"),
                upvotes: r.get("upvotes"),
                downvotes: r.get("downvotes"),
                updated_at: r.get("updated_at"),
                already_notified: r.get("already_notified"),
            })
            .collect())
    }

    /// Read a named poll cursor. Returns `None` if the poller has never run.
    #[instrument(name = "notification_indexer.storage.get_poll_cursor", skip(self))]
    pub async fn get_poll_cursor(
        &self,
        name: &str,
    ) -> Result<Option<(DateTime<Utc>, i64)>, StorageError> {
        let row = sqlx::query(
            "SELECT cursor_updated_at, cursor_id FROM notification_poll_cursors WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.get("cursor_updated_at"), r.get("cursor_id"))))
    }

    /// Persist a named poll cursor (high-water `(updated_at, id)`).
    #[instrument(name = "notification_indexer.storage.set_poll_cursor", skip(self))]
    pub async fn set_poll_cursor(
        &self,
        name: &str,
        cursor_updated_at: DateTime<Utc>,
        cursor_id: i64,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO notification_poll_cursors (name, cursor_updated_at, cursor_id, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (name) DO UPDATE SET
                cursor_updated_at = EXCLUDED.cursor_updated_at,
                cursor_id = EXCLUDED.cursor_id,
                updated_at = now()
            "#,
        )
        .bind(name)
        .bind(cursor_updated_at)
        .bind(cursor_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
