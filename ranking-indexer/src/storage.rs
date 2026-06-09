//! PostgreSQL persistence for the private `ranks` working schema.
//!
//! All writes are upserts keyed deterministically so reprocessing an edit
//! converges on the same state (idempotency, design §10). Uses runtime
//! `sqlx::query` (not the compile-time macro) so the crate builds without a
//! live database.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::{Ranking, RankingBlock, RankingItem};

#[derive(Clone)]
pub struct Storage {
    pool: PgPool,
}

impl Storage {
    pub async fn new(database_url: &str) -> Result<Self, IndexerError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Upsert a Ranking Block.
    pub async fn upsert_ranking_block(&self, b: &RankingBlock) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO ranks.ranking_blocks
                (id, space_id, name, filter, start_date, end_date, restriction_id, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, now())
            ON CONFLICT (id) DO UPDATE SET
                space_id = EXCLUDED.space_id,
                name = EXCLUDED.name,
                filter = EXCLUDED.filter,
                start_date = EXCLUDED.start_date,
                end_date = EXCLUDED.end_date,
                restriction_id = EXCLUDED.restriction_id,
                updated_at = now()
            "#,
        )
        .bind(b.id)
        .bind(b.space_id)
        .bind(&b.name)
        .bind(&b.filter)
        .bind(b.start_date)
        .bind(b.end_date)
        .bind(b.restriction_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert a Rank submission. A null incoming `block_id` never clobbers an
    /// already-known link (the link may have arrived in an earlier edit).
    pub async fn upsert_ranking(&self, r: &Ranking) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO ranks.rankings
                (id, block_id, space_id, author_address, rank_type, submitted_at,
                 updated_at_block, update_index, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
            ON CONFLICT (id) DO UPDATE SET
                block_id = COALESCE(EXCLUDED.block_id, ranks.rankings.block_id),
                space_id = EXCLUDED.space_id,
                author_address = EXCLUDED.author_address,
                rank_type = EXCLUDED.rank_type,
                submitted_at = EXCLUDED.submitted_at,
                updated_at_block = EXCLUDED.updated_at_block,
                update_index = EXCLUDED.update_index,
                updated_at = now()
            "#,
        )
        .bind(r.id)
        .bind(r.block_id)
        .bind(r.space_id)
        .bind(&r.author_address)
        .bind(&r.rank_type)
        .bind(r.submitted_at)
        .bind(r.updated_at_block)
        .bind(r.update_index)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Set a rank's block link (the `RANK_BLOCK` relation may land in a later edit).
    pub async fn set_ranking_block(
        &self,
        ranking_id: Uuid,
        block_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query("UPDATE ranks.rankings SET block_id = $2, updated_at = now() WHERE id = $1")
            .bind(ranking_id)
            .bind(block_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Replace a submission's items wholesale (re-submission rebuilds them).
    pub async fn replace_ranking_items(
        &self,
        ranking_id: Uuid,
        items: &[RankingItem],
    ) -> Result<(), IndexerError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM ranks.ranking_items WHERE ranking_id = $1")
            .bind(ranking_id)
            .execute(&mut *tx)
            .await?;
        for it in items {
            sqlx::query(
                r#"
                INSERT INTO ranks.ranking_items (ranking_id, entity_id, space_id, position, weight)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(it.ranking_id)
            .bind(it.entity_id)
            .bind(it.space_id)
            .bind(&it.position)
            .bind(it.weight)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
