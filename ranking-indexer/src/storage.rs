//! PostgreSQL persistence for the private `ranks` working schema.
//!
//! All writes are upserts keyed deterministically so reprocessing an edit
//! converges on the same state (idempotency, design §10). Uses runtime
//! `sqlx::query` (not the compile-time macro) so the crate builds without a
//! live database.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

use crate::eligibility::SpaceKind;
use crate::error::IndexerError;
use crate::models::{Ranking, RankingBlock, RankingItem};
use crate::publish::{provenance_ids, RankPositionRow};
use crate::scoring::ScoreRow;

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

    /// Resolve a space's kind from `public.spaces.type` (`DAO` / `Personal`).
    /// Returns `None` if the space isn't known yet; callers treat unknown
    /// conservatively (as `Dao`, i.e. membership-restricted).
    pub async fn space_kind(&self, space_id: Uuid) -> Result<Option<SpaceKind>, IndexerError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT type::text FROM spaces WHERE id = $1")
            .bind(space_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(t,)| match t.as_str() {
            "Personal" => SpaceKind::Personal,
            _ => SpaceKind::Dao,
        }))
    }

    /// The set of personal-space ids that are members OR editors of `space_id`
    /// — the eligible voters for a DAO-space block. Reads the indexer's own
    /// view (`ranks.members` / `ranks.editors`, fed from `space.membership`)
    /// rather than the kg-indexer-maintained public tables, so a recompute
    /// never races the kg-indexer's consumer group.
    pub async fn member_and_editor_spaces(
        &self,
        space_id: Uuid,
    ) -> Result<HashSet<Uuid>, IndexerError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT member_space_id FROM ranks.members WHERE space_id = $1
             UNION
             SELECT member_space_id FROM ranks.editors WHERE space_id = $1",
        )
        .bind(space_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Record `member_space_id` as a member of `space_id` in the view.
    pub async fn add_member(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            "INSERT INTO ranks.members (member_space_id, space_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(member_space_id)
        .bind(space_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop `member_space_id` as a member of `space_id` from the view.
    pub async fn remove_member(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query("DELETE FROM ranks.members WHERE member_space_id = $1 AND space_id = $2")
            .bind(member_space_id)
            .bind(space_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record `member_space_id` as an editor of `space_id` in the view.
    pub async fn add_editor(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            "INSERT INTO ranks.editors (member_space_id, space_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(member_space_id)
        .bind(space_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Drop `member_space_id` as an editor of `space_id` from the view.
    pub async fn remove_editor(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query("DELETE FROM ranks.editors WHERE member_space_id = $1 AND space_id = $2")
            .bind(member_space_id)
            .bind(space_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Drop both roles at once (`SPACE_LEFT` carries no role).
    pub async fn remove_member_and_editor(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<(), IndexerError> {
        let mut tx = self.pool.begin().await?;
        for table in ["ranks.members", "ranks.editors"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE member_space_id = $1 AND space_id = $2"
            ))
            .bind(member_space_id)
            .bind(space_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Blocks whose aggregate a membership change for `(space_id,
    /// member_space_id)` can affect: blocks in the affected space holding a
    /// submission from that member's personal space.
    pub async fn blocks_with_rankings_from(
        &self,
        space_id: Uuid,
        member_space_id: Uuid,
    ) -> Result<Vec<Uuid>, IndexerError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT r.block_id
             FROM ranks.rankings r
             JOIN ranks.ranking_blocks b ON b.id = r.block_id AND b.space_id = $1
             WHERE r.space_id = $2 AND r.block_id IS NOT NULL",
        )
        .bind(space_id)
        .bind(member_space_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// The block a rank is currently linked to (the link may have been set in an
    /// earlier edit).
    pub async fn block_id_for_ranking(
        &self,
        ranking_id: Uuid,
    ) -> Result<Option<Uuid>, IndexerError> {
        let row: Option<(Option<Uuid>,)> =
            sqlx::query_as("SELECT block_id FROM ranks.rankings WHERE id = $1")
                .bind(ranking_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(block_id,)| block_id))
    }

    /// Load a Ranking Block, if known.
    pub async fn get_ranking_block(
        &self,
        block_id: Uuid,
    ) -> Result<Option<RankingBlock>, IndexerError> {
        let block = sqlx::query_as::<_, RankingBlock>(
            "SELECT id, space_id, name, filter, start_date, end_date, restriction_id \
             FROM ranks.ranking_blocks WHERE id = $1",
        )
        .bind(block_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(block)
    }

    /// Reconstruct a Ranking Block's config from the indexed public graph.
    ///
    /// `detect()` only registers a block when its `TYPES -> Ranking Block`
    /// relation and its config (Name/Filter/dates/restriction) arrive in the
    /// *same* edit, because it resolves an entity's types from the current edit
    /// alone. Real clients emit the type and the config across separate edits,
    /// so the block is never registered and every rank linked to it is silently
    /// never scored (issue #738). When a rank links to a block we never
    /// registered, recover it from the graph here. Returns `None` if the entity
    /// is not (yet) typed as a Ranking Block in `public.relations`.
    pub async fn get_block_config_from_kg(
        &self,
        block_id: Uuid,
    ) -> Result<Option<RankingBlock>, IndexerError> {
        use sdk::core::ids;
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");

        // Must be typed as a Ranking Block in the graph; the type relation also
        // tells us the block's home space.
        let typed: Option<(Uuid,)> = sqlx::query_as(
            "SELECT space_id FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 AND to_entity_id = $3 \
             LIMIT 1",
        )
        .bind(block_id)
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANKING_BLOCK_TYPE_ID))
        .fetch_optional(&self.pool)
        .await?;
        let Some((space_id,)) = typed else {
            return Ok(None);
        };

        // Config properties: Name/Filter as text, Start/End as datetime. One row
        // is always returned (aggregates over zero matching values yield NULLs).
        type ConfigRow = (
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        );
        // Scope to the block's home space: the same entity may be perspectived
        // into other spaces with different config, keyed on (id, space_id).
        let (name, filter, start_date, end_date): ConfigRow = sqlx::query_as(
            "SELECT \
               max(text)         FILTER (WHERE property_id = $2) AS name, \
               max(text)         FILTER (WHERE property_id = $3) AS filter, \
               max(datetime_utc) FILTER (WHERE property_id = $4) AS start_date, \
               max(datetime_utc) FILTER (WHERE property_id = $5) AS end_date \
             FROM values WHERE entity_id = $1 AND space_id = $6",
        )
        .bind(block_id)
        .bind(pid(ids::NAME_PROPERTY_ID))
        .bind(pid(ids::RANK_FILTER_PROPERTY_ID))
        .bind(pid(ids::RANK_START_DATE_PROPERTY_ID))
        .bind(pid(ids::RANK_END_DATE_PROPERTY_ID))
        .bind(space_id)
        .fetch_one(&self.pool)
        .await?;

        // Aggregation restriction is a relation (as detect() collects it),
        // likewise scoped to the block's home space.
        let restriction: Option<(Uuid,)> = sqlx::query_as(
            "SELECT to_entity_id FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 AND space_id = $3 LIMIT 1",
        )
        .bind(block_id)
        .bind(pid(ids::RANK_AGGREGATION_RESTRICTION_PROPERTY_ID))
        .bind(space_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(Some(RankingBlock {
            id: block_id,
            space_id,
            name,
            filter,
            start_date,
            end_date,
            restriction_id: restriction.map(|(r,)| r),
        }))
    }

    /// Load all submissions currently linked to a block (pre-dedup).
    pub async fn get_rankings_for_block(
        &self,
        block_id: Uuid,
    ) -> Result<Vec<Ranking>, IndexerError> {
        let rows = sqlx::query_as::<_, Ranking>(
            "SELECT id, block_id, space_id, author_address, rank_type, submitted_at, \
             updated_at_block, update_index FROM ranks.rankings WHERE block_id = $1",
        )
        .bind(block_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Upsert a Ranking Block.
    pub async fn upsert_ranking_block(&self, b: &RankingBlock) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO ranks.ranking_blocks
                (id, space_id, name, filter, start_date, end_date, restriction_id, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, now())
            ON CONFLICT (id, space_id) DO UPDATE SET
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
            ON CONFLICT (id, space_id) DO UPDATE SET
                block_id = COALESCE(EXCLUDED.block_id, ranks.rankings.block_id),
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
    /// `ranks.rankings` is keyed on `(id, space_id)`, so the update is scoped to
    /// the rank's space — a same-id rank perspectived into another space is left
    /// untouched.
    pub async fn set_ranking_block(
        &self,
        ranking_id: Uuid,
        block_id: Uuid,
        space_id: Uuid,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            "UPDATE ranks.rankings SET block_id = $2, updated_at = now() \
             WHERE id = $1 AND space_id = $3",
        )
        .bind(ranking_id)
        .bind(block_id)
        .bind(space_id)
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

    /// Load the items of a set of rankings (the eligible submissions for a block).
    pub async fn get_items_for_rankings(
        &self,
        ranking_ids: &[Uuid],
    ) -> Result<Vec<RankingItem>, IndexerError> {
        if ranking_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, RankingItem>(
            "SELECT ranking_id, entity_id, space_id, position, weight \
             FROM ranks.ranking_items WHERE ranking_id = ANY($1)",
        )
        .bind(ranking_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Replace a block's computed aggregate wholesale (atomic full recompute).
    pub async fn replace_ranking_scores(
        &self,
        block_id: Uuid,
        rows: &[ScoreRow],
    ) -> Result<(), IndexerError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM ranks.ranking_scores WHERE block_id = $1")
            .bind(block_id)
            .execute(&mut *tx)
            .await?;
        for r in rows {
            sqlx::query(
                "INSERT INTO ranks.ranking_scores (block_id, entity_id, space_id, score, position) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(block_id)
            .bind(r.entity_id)
            .bind(r.space_id)
            .bind(r.score)
            .bind(r.position)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replace a block's public `RANK_POSITION` projection atomically:
    /// the ordered relations (with the integer rank-position value on each
    /// reified entity) and the `Aggregated rankings` provenance relations.
    pub async fn replace_rank_position_projection(
        &self,
        block_id: Uuid,
        block_space_id: Uuid,
        rows: &[RankPositionRow],
        contributing_rankings: &[Uuid],
    ) -> Result<(), IndexerError> {
        use sdk::core::ids::{
            AGGREGATED_RANKINGS_RELATION_TYPE_ID, RANK_POSITION_RELATION_TYPE_ID,
            RANK_POSITION_VALUE_PROPERTY_ID,
        };
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");
        let rank_position = pid(RANK_POSITION_RELATION_TYPE_ID);
        let aggregated = pid(AGGREGATED_RANKINGS_RELATION_TYPE_ID);
        let value_prop = pid(RANK_POSITION_VALUE_PROPERTY_ID);

        let mut tx = self.pool.begin().await?;

        // 1. Drop prior value rows on this block's reified RANK_POSITION entities
        //    (before the relations they hang off are deleted). Scoped to this
        //    block's space so a same-id block perspectived into another space
        //    keeps its own projection.
        sqlx::query(
            "DELETE FROM values WHERE property_id = $1 AND space_id = $4 AND entity_id IN \
             (SELECT entity_id FROM relations \
              WHERE type_id = $2 AND from_entity_id = $3 AND space_id = $4)",
        )
        .bind(value_prop)
        .bind(rank_position)
        .bind(block_id)
        .bind(block_space_id)
        .execute(&mut *tx)
        .await?;

        // 2. Drop prior projection relations (RANK_POSITION + Aggregated rankings).
        sqlx::query(
            "DELETE FROM relations WHERE from_entity_id = $1 AND type_id = ANY($2) AND space_id = $3",
        )
        .bind(block_id)
        .bind(&[rank_position, aggregated][..])
        .bind(block_space_id)
        .execute(&mut *tx)
        .await?;

        // 3. Insert the ordered RANK_POSITION relations + their value rows.
        for r in rows {
            sqlx::query(
                "INSERT INTO relations \
                 (id, entity_id, type_id, from_entity_id, to_entity_id, to_space_id, space_id, position) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(r.relation_id)
            .bind(r.reified_entity_id)
            .bind(rank_position)
            .bind(block_id)
            .bind(r.entity_id)
            .bind(r.space_id)
            .bind(block_space_id)
            .bind(&r.position)
            .execute(&mut *tx)
            .await?;

            // `values.id` is a text column — serialize the UUID at the bind boundary.
            sqlx::query(
                "INSERT INTO values (id, entity_id, space_id, property_id, integer) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(r.value_row_id.to_string())
            .bind(r.reified_entity_id)
            .bind(block_space_id)
            .bind(value_prop)
            .bind(r.value)
            .execute(&mut *tx)
            .await?;
        }

        // 4. Insert Aggregated rankings provenance relations (block -> submission).
        for ranking_id in contributing_rankings {
            let (relation_id, reified) = provenance_ids(block_id, *ranking_id);
            sqlx::query(
                "INSERT INTO relations \
                 (id, entity_id, type_id, from_entity_id, to_entity_id, space_id) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(relation_id)
            .bind(reified)
            .bind(aggregated)
            .bind(block_id)
            .bind(ranking_id)
            .bind(block_space_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
