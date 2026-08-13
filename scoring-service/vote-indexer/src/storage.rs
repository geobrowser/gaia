//! Storage layer for vote data.

use hermes_instrumentation::instrument;
use sdk::core::ids::SCORE_PROPERTY_ID;
use sqlx::{PgPool, Postgres, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::models::voting::{
    ResponseKind, ScoreValueItem, UserVoteCriteria, UserVoteItem, VoteCountCriteria, VoteItem,
    VoteObjectType, VotesCountItem,
};

/// Storage for vote-related database operations.
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

    /// Insert raw vote records.
    ///
    /// Stores every vote event.
    /// Each vote creates a new row.
    #[instrument(name = "vote_indexer.storage.insert_votes", skip(self, votes, tx), fields(count = votes.len()))]
    pub async fn insert_votes(
        &self,
        votes: &[VoteItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), StorageError> {
        if votes.is_empty() {
            return Ok(());
        }

        let mut voter_ids = Vec::with_capacity(votes.len());
        let mut object_ids = Vec::with_capacity(votes.len());
        let mut object_types = Vec::with_capacity(votes.len());
        let mut space_ids = Vec::with_capacity(votes.len());
        let mut vote_values = Vec::with_capacity(votes.len());
        let mut vote_kinds = Vec::with_capacity(votes.len());
        let mut block_numbers = Vec::with_capacity(votes.len());
        let mut block_timestamps = Vec::with_capacity(votes.len());

        for vote in votes {
            voter_ids.push(vote.voter_id);
            object_ids.push(vote.object_id);
            object_types.push(i16::from(vote.object_type));
            space_ids.push(vote.space_id);
            vote_values.push(i16::from(vote.vote));
            vote_kinds.push(i16::from(vote.kind));
            block_numbers.push(vote.block_number as i64);
            block_timestamps.push(vote.block_timestamp as i64);
        }

        // vote_kind is stored here too: this log keeps a decoded direction, never
        // the action hash, so without it a Remove row cannot say which axis it
        // cleared and the log stops being replayable into current state.
        let query = r#"
            INSERT INTO votes (voter_id, object_id, object_type, space_id, vote, vote_kind, block_number, block_timestamp)
            SELECT voter_id, object_id, object_type, space_id, vote, vote_kind, block_number, to_timestamp(block_timestamp)
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[], $5::smallint[], $6::smallint[], $7::bigint[], $8::bigint[]
            ) AS t(voter_id, object_id, object_type, space_id, vote, vote_kind, block_number, block_timestamp)
        "#;

        sqlx::query(query)
            .bind(&voter_ids)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_values)
            .bind(&vote_kinds)
            .bind(&block_numbers)
            .bind(&block_timestamps)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Upsert user vote records.
    ///
    /// Updates the user's current vote for each object/space combination.
    /// Uses ON CONFLICT to update existing votes.
    #[instrument(name = "vote_indexer.storage.upsert_user_votes", skip(self, votes, tx), fields(count = votes.len()))]
    pub async fn upsert_user_votes(
        &self,
        votes: &[UserVoteItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), StorageError> {
        if votes.is_empty() {
            return Ok(());
        }

        let mut voter_ids = Vec::with_capacity(votes.len());
        let mut object_ids = Vec::with_capacity(votes.len());
        let mut object_types = Vec::with_capacity(votes.len());
        let mut space_ids = Vec::with_capacity(votes.len());
        let mut vote_types = Vec::with_capacity(votes.len());
        let mut vote_kinds = Vec::with_capacity(votes.len());
        let mut voted_ats = Vec::with_capacity(votes.len());

        for vote in votes {
            voter_ids.push(vote.voter_id);
            object_ids.push(vote.object_id);
            object_types.push(i16::from(vote.object_type));
            space_ids.push(vote.space_id);
            vote_types.push(i16::from(vote.vote_type));
            vote_kinds.push(i16::from(vote.kind));
            voted_ats.push(vote.voted_at as i64);
        }

        // vote_kind is in the conflict target, which is what keeps the axes
        // independent. Drop it and a Verify would overwrite the same user's
        // upvote on the same object instead of sitting alongside it.
        let query = r#"
            INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
            SELECT user_id, object_id, object_type, space_id, vote_type, vote_kind,
                   to_timestamp(voted_at)
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[], $5::smallint[], $6::smallint[], $7::bigint[]
            ) AS t(user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at)
            ON CONFLICT (user_id, object_id, object_type, space_id, vote_kind)
            DO UPDATE SET
                vote_type = EXCLUDED.vote_type,
                voted_at = EXCLUDED.voted_at
        "#;

        sqlx::query(query)
            .bind(&voter_ids)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_types)
            .bind(&vote_kinds)
            .bind(&voted_ats)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Upsert aggregated vote counts.
    ///
    /// Updates the positive/negative tallies for each object/space/kind
    /// combination. Uses ON CONFLICT to update existing counts.
    #[instrument(name = "vote_indexer.storage.upsert_votes_counts", skip(self, counts, tx), fields(count = counts.len()))]
    pub async fn upsert_votes_counts(
        &self,
        counts: &[VotesCountItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), StorageError> {
        if counts.is_empty() {
            return Ok(());
        }

        let mut object_ids = Vec::with_capacity(counts.len());
        let mut object_types = Vec::with_capacity(counts.len());
        let mut space_ids = Vec::with_capacity(counts.len());
        let mut vote_kinds = Vec::with_capacity(counts.len());
        let mut positives = Vec::with_capacity(counts.len());
        let mut negatives = Vec::with_capacity(counts.len());

        for count in counts {
            object_ids.push(count.object_id);
            object_types.push(i16::from(count.object_type));
            space_ids.push(count.space_id);
            vote_kinds.push(i16::from(count.kind));
            positives.push(count.positive);
            negatives.push(count.negative);
        }

        // Writes target positive/negative. The upvotes/downvotes columns still
        // exist for the pre-vote_kind client but are GENERATED ALWAYS, so naming
        // them here would be rejected outright.
        //
        // updated_at uses clock_timestamp() (the statement's wall-clock time, closer
        // to the actual write) rather than now() (fixed at transaction start).
        let query = r#"
            INSERT INTO votes_count (object_id, object_type, space_id, vote_kind, positive, negative, updated_at)
            SELECT object_id, object_type, space_id, vote_kind, positive, negative, clock_timestamp()
            FROM UNNEST(
                $1::uuid[], $2::smallint[], $3::uuid[], $4::smallint[], $5::bigint[], $6::bigint[]
            ) AS t(object_id, object_type, space_id, vote_kind, positive, negative)
            ON CONFLICT (object_id, object_type, space_id, vote_kind)
            DO UPDATE SET
                positive = EXCLUDED.positive,
                negative = EXCLUDED.negative,
                updated_at = clock_timestamp()
        "#;

        sqlx::query(query)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_kinds)
            .bind(&positives)
            .bind(&negatives)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Recompute the Explore feed ranking score for entities whose curation votes
    /// just changed.
    ///
    /// Runs OUTSIDE the vote transaction, on the pool, and reports failure to the
    /// caller as a warning rather than an error. Two reasons:
    ///
    /// 1. A stale ranking score is harmless and self-correcting — the next vote or a
    ///    backfill run fixes it. A lost vote is permanent. So the refresh must never
    ///    be able to abort the vote write, whatever goes wrong.
    /// 2. It makes deploy order irrelevant. If vote-indexer ships ahead of the
    ///    migration that creates the function, this logs and moves on instead of
    ///    failing every vote write — the failure shape of the 08-05 incident, where a
    ///    migration shipped without its writer.
    ///
    /// A catalogue guard (`WHERE to_regproc(...) IS NOT NULL`) does NOT work here and
    /// was tried: Postgres resolves the function name at parse time, so the statement
    /// fails before the WHERE is ever evaluated. Separating the transaction is what
    /// actually provides the safety.
    ///
    /// Only curation (`vote_kind = 0`) on entities feeds the feed's quality term, so
    /// stance and veracity votes do not trigger a recompute — they would rewrite the
    /// row to an identical value and make `updated_at` misleading about what moved.
    ///
    /// Because the score is time-invariant (`created_at / tau`), this is the only
    /// thing that ever needs to run. There is no scheduled decay sweep.
    #[instrument(
        name = "vote_indexer.storage.refresh_ranking_scores",
        skip(self, counts)
    )]
    pub async fn refresh_ranking_scores(
        &self,
        counts: &[VotesCountItem],
    ) -> Result<u64, StorageError> {
        let mut entity_ids: Vec<Uuid> = counts
            .iter()
            .filter(|c| c.object_type == VoteObjectType::Entity && c.kind == ResponseKind::Curation)
            .map(|c| c.object_id)
            .collect();
        // An entity voted in several spaces appears once per space; deduplicate so a
        // single recompute covers it.
        entity_ids.sort_unstable();
        entity_ids.dedup();

        if entity_ids.is_empty() {
            return Ok(0);
        }

        let scored: i32 =
            sqlx::query_scalar("SELECT public.refresh_entity_ranking_scores($1::uuid[])")
                .bind(&entity_ids)
                .fetch_one(&self.pool)
                .await?;

        Ok(scored.max(0) as u64)
    }

    /// Mirror net scores into the `values` table under the Score system property.
    ///
    /// Enables sorting entities by `positive - negative` through the existing
    /// `entities_ordered_by_property` function without any SQL changes.
    /// Curation only — see `build_score_values`, which filters the other kinds
    /// out before they reach here.
    /// Upserts on `id` — a deterministic UUIDv5 of `score:<entity>:<space>`
    /// under `GEO_SYSTEM_NAMESPACE`, stored as text. The `score:` tag keeps
    /// these ids disjoint from kg-indexer-minted value ids and any other
    /// `(entity_id, space_id)` scheme.
    #[instrument(name = "vote_indexer.storage.upsert_score_values", skip(self, rows, tx), fields(count = rows.len()))]
    pub async fn upsert_score_values(
        &self,
        rows: &[ScoreValueItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), StorageError> {
        if rows.is_empty() {
            return Ok(());
        }

        let property_id =
            Uuid::parse_str(SCORE_PROPERTY_ID).expect("SCORE_PROPERTY_ID is a valid UUID constant");

        let mut ids = Vec::with_capacity(rows.len());
        let mut entity_ids = Vec::with_capacity(rows.len());
        let mut space_ids = Vec::with_capacity(rows.len());
        let mut integers = Vec::with_capacity(rows.len());

        for row in rows {
            // `values.id` column is text, so serialize the UUID at the bind boundary.
            ids.push(row.id.to_string());
            entity_ids.push(row.entity_id);
            space_ids.push(row.space_id);
            integers.push(row.integer);
        }

        let query = r#"
            INSERT INTO values (id, entity_id, space_id, property_id, integer)
            SELECT id, entity_id, space_id, $5::uuid, integer
            FROM UNNEST(
                $1::text[], $2::uuid[], $3::uuid[], $4::bigint[]
            ) AS t(id, entity_id, space_id, integer)
            ON CONFLICT (id) DO UPDATE SET
                integer = EXCLUDED.integer,
                property_id = EXCLUDED.property_id,
                entity_id = EXCLUDED.entity_id,
                space_id = EXCLUDED.space_id
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&entity_ids)
            .bind(&space_ids)
            .bind(&integers)
            .bind(property_id)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Get existing user votes for the given criteria within a transaction.
    ///
    /// Used to calculate vote deltas when processing new votes.
    /// Uses FOR UPDATE to lock rows and prevent concurrent modifications.
    #[instrument(name = "vote_indexer.storage.get_user_votes_tx", skip(self, criteria, tx), fields(criteria_count = criteria.len()))]
    pub async fn get_user_votes_tx(
        &self,
        criteria: &[UserVoteCriteria],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<Vec<UserVoteItem>, StorageError> {
        if criteria.is_empty() {
            return Ok(Vec::new());
        }

        let voter_ids: Vec<Uuid> = criteria.iter().map(|(v, _, _, _, _)| *v).collect();
        let object_ids: Vec<Uuid> = criteria.iter().map(|(_, o, _, _, _)| *o).collect();
        let space_ids: Vec<Uuid> = criteria.iter().map(|(_, _, s, _, _)| *s).collect();
        let object_types: Vec<i16> = criteria
            .iter()
            .map(|(_, _, _, t, _)| i16::from(*t))
            .collect();
        let vote_kinds: Vec<i16> = criteria
            .iter()
            .map(|(_, _, _, _, k)| i16::from(*k))
            .collect();

        // vote_kind is part of the lookup key. Without it this would lock and
        // return a user's rows across all three axes, and the delta computed
        // against the wrong one would corrupt the tallies.
        let query = r#"
            SELECT user_id, object_id, object_type, space_id, vote_type, vote_kind, voted_at
            FROM user_votes
            WHERE (user_id, object_id, object_type, space_id, vote_kind) IN (
                SELECT user_id, object_id, object_type, space_id, vote_kind
                FROM UNNEST($1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[], $5::smallint[])
                AS t(user_id, object_id, object_type, space_id, vote_kind)
            )
            FOR UPDATE
        "#;

        let rows = sqlx::query(query)
            .bind(&voter_ids)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_kinds)
            .fetch_all(&mut **tx)
            .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let voted_at: chrono::DateTime<chrono::Utc> = row.get("voted_at");
            let object_type: i16 = row.get("object_type");
            let vote_type: i16 = row.get("vote_type");
            let vote_kind: i16 = row.get("vote_kind");
            result.push(UserVoteItem {
                voter_id: row.get("user_id"),
                object_id: row.get("object_id"),
                object_type: object_type.into(),
                space_id: row.get("space_id"),
                vote_type: vote_type.into(),
                kind: vote_kind.into(),
                voted_at: voted_at.timestamp() as u64,
            });
        }

        Ok(result)
    }

    /// Get existing vote counts for the given criteria within a transaction.
    ///
    /// Used to calculate updated counts when processing new votes.
    /// Uses FOR UPDATE to lock rows and prevent concurrent modifications.
    #[instrument(name = "vote_indexer.storage.get_votes_counts_tx", skip(self, criteria, tx), fields(criteria_count = criteria.len()))]
    pub async fn get_votes_counts_tx(
        &self,
        criteria: &[VoteCountCriteria],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<Vec<VotesCountItem>, StorageError> {
        if criteria.is_empty() {
            return Ok(Vec::new());
        }

        let object_ids: Vec<Uuid> = criteria.iter().map(|(o, _, _, _)| *o).collect();
        let space_ids: Vec<Uuid> = criteria.iter().map(|(_, s, _, _)| *s).collect();
        let object_types: Vec<i16> = criteria.iter().map(|(_, _, t, _)| i16::from(*t)).collect();
        let vote_kinds: Vec<i16> = criteria.iter().map(|(_, _, _, k)| i16::from(*k)).collect();

        let query = r#"
            SELECT object_id, object_type, space_id, vote_kind, positive, negative
            FROM votes_count
            WHERE (object_id, object_type, space_id, vote_kind) IN (
                SELECT object_id, object_type, space_id, vote_kind
                FROM UNNEST($1::uuid[], $2::smallint[], $3::uuid[], $4::smallint[])
                AS t(object_id, object_type, space_id, vote_kind)
            )
            FOR UPDATE
        "#;

        let rows = sqlx::query(query)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_kinds)
            .fetch_all(&mut **tx)
            .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let object_type: i16 = row.get("object_type");
            let vote_kind: i16 = row.get("vote_kind");
            result.push(VotesCountItem {
                object_id: row.get("object_id"),
                object_type: object_type.into(),
                space_id: row.get("space_id"),
                kind: vote_kind.into(),
                positive: row.get("positive"),
                negative: row.get("negative"),
            });
        }

        Ok(result)
    }
}
