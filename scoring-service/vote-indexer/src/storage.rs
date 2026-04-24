//! Storage layer for vote data.

use hermes_instrumentation::instrument;
use sdk::core::ids::SCORE_PROPERTY_ID;
use sqlx::{PgPool, Postgres, Row};
use uuid::Uuid;

use crate::error::StorageError;
use crate::models::voting::{
    ScoreValueItem, UserVoteCriteria, UserVoteItem, VoteCountCriteria, VoteItem, VotesCountItem,
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
        let mut block_numbers = Vec::with_capacity(votes.len());
        let mut block_timestamps = Vec::with_capacity(votes.len());

        for vote in votes {
            voter_ids.push(vote.voter_id);
            object_ids.push(vote.object_id);
            object_types.push(i16::from(vote.object_type));
            space_ids.push(vote.space_id);
            vote_values.push(i16::from(vote.vote));
            block_numbers.push(vote.block_number as i64);
            block_timestamps.push(vote.block_timestamp as i64);
        }

        let query = r#"
            INSERT INTO votes (voter_id, object_id, object_type, space_id, vote, block_number, block_timestamp)
            SELECT voter_id, object_id, object_type, space_id, vote, block_number, to_timestamp(block_timestamp)
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[], $5::smallint[], $6::bigint[], $7::bigint[]
            ) AS t(voter_id, object_id, object_type, space_id, vote, block_number, block_timestamp)
        "#;

        sqlx::query(query)
            .bind(&voter_ids)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&vote_values)
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
        let mut voted_ats = Vec::with_capacity(votes.len());

        for vote in votes {
            voter_ids.push(vote.voter_id);
            object_ids.push(vote.object_id);
            object_types.push(i16::from(vote.object_type));
            space_ids.push(vote.space_id);
            vote_types.push(i16::from(vote.vote_type));
            voted_ats.push(vote.voted_at as i64);
        }

        let query = r#"
            INSERT INTO user_votes (user_id, object_id, object_type, space_id, vote_type, voted_at)
            SELECT user_id, object_id, object_type, space_id, vote_type,
                   to_timestamp(voted_at)
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[], $5::smallint[], $6::bigint[]
            ) AS t(user_id, object_id, object_type, space_id, vote_type, voted_at)
            ON CONFLICT (user_id, object_id, object_type, space_id)
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
            .bind(&voted_ats)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Upsert aggregated vote counts.
    ///
    /// Updates the total upvotes/downvotes for each object/space combination.
    /// Uses ON CONFLICT to update existing counts.
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
        let mut upvotes = Vec::with_capacity(counts.len());
        let mut downvotes = Vec::with_capacity(counts.len());

        for count in counts {
            object_ids.push(count.object_id);
            object_types.push(i16::from(count.object_type));
            space_ids.push(count.space_id);
            upvotes.push(count.upvotes);
            downvotes.push(count.downvotes);
        }

        let query = r#"
            INSERT INTO votes_count (object_id, object_type, space_id, upvotes, downvotes)
            SELECT object_id, object_type, space_id, upvotes, downvotes
            FROM UNNEST(
                $1::uuid[], $2::smallint[], $3::uuid[], $4::bigint[], $5::bigint[]
            ) AS t(object_id, object_type, space_id, upvotes, downvotes)
            ON CONFLICT (object_id, object_type, space_id)
            DO UPDATE SET
                upvotes = EXCLUDED.upvotes,
                downvotes = EXCLUDED.downvotes
        "#;

        sqlx::query(query)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .bind(&upvotes)
            .bind(&downvotes)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Mirror net scores into the `values` table under the Score system property.
    ///
    /// Enables sorting entities by `upvotes - downvotes` through the existing
    /// `entities_ordered_by_property` function without any SQL changes.
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

        let property_id = Uuid::parse_str(SCORE_PROPERTY_ID)
            .expect("SCORE_PROPERTY_ID is a valid UUID constant");

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

        let voter_ids: Vec<Uuid> = criteria.iter().map(|(v, _, _, _)| *v).collect();
        let object_ids: Vec<Uuid> = criteria.iter().map(|(_, o, _, _)| *o).collect();
        let space_ids: Vec<Uuid> = criteria.iter().map(|(_, _, s, _)| *s).collect();
        let object_types: Vec<i16> = criteria.iter().map(|(_, _, _, t)| i16::from(*t)).collect();

        let query = r#"
            SELECT user_id, object_id, object_type, space_id, vote_type, voted_at
            FROM user_votes
            WHERE (user_id, object_id, object_type, space_id) IN (
                SELECT user_id, object_id, object_type, space_id
                FROM UNNEST($1::uuid[], $2::uuid[], $3::smallint[], $4::uuid[])
                AS t(user_id, object_id, object_type, space_id)
            )
            FOR UPDATE
        "#;

        let rows = sqlx::query(query)
            .bind(&voter_ids)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .fetch_all(&mut **tx)
            .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let voted_at: chrono::DateTime<chrono::Utc> = row.get("voted_at");
            let object_type: i16 = row.get("object_type");
            let vote_type: i16 = row.get("vote_type");
            result.push(UserVoteItem {
                voter_id: row.get("user_id"),
                object_id: row.get("object_id"),
                object_type: object_type.into(),
                space_id: row.get("space_id"),
                vote_type: vote_type.into(),
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

        let object_ids: Vec<Uuid> = criteria.iter().map(|(o, _, _)| *o).collect();
        let space_ids: Vec<Uuid> = criteria.iter().map(|(_, s, _)| *s).collect();
        let object_types: Vec<i16> = criteria.iter().map(|(_, _, t)| i16::from(*t)).collect();

        let query = r#"
            SELECT object_id, object_type, space_id, upvotes, downvotes
            FROM votes_count
            WHERE (object_id, object_type, space_id) IN (
                SELECT object_id, object_type, space_id
                FROM UNNEST($1::uuid[], $2::smallint[], $3::uuid[])
                AS t(object_id, object_type, space_id)
            )
            FOR UPDATE
        "#;

        let rows = sqlx::query(query)
            .bind(&object_ids)
            .bind(&object_types)
            .bind(&space_ids)
            .fetch_all(&mut **tx)
            .await?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let object_type: i16 = row.get("object_type");
            result.push(VotesCountItem {
                object_id: row.get("object_id"),
                object_type: object_type.into(),
                space_id: row.get("space_id"),
                upvotes: row.get("upvotes"),
                downvotes: row.get("downvotes"),
            });
        }

        Ok(result)
    }
}
