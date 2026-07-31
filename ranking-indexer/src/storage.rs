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
use crate::models::{BlockMeta, Ranking, RankingBlock, RankingItem};
use crate::publish::{provenance_ids, RankPositionRow};
use crate::scoring::ScoreRow;

/// Resolve one submission-window bound from separately queried legacy/datetime
/// columns: the datetime property (GEO-2253) wins per-bound; the legacy date
/// property is the fallback. Mirrors `detect::window_bound`'s precedence so
/// the live and KG-recovery paths never disagree on a block that authors both.
fn resolve_window_bound(
    block: Uuid,
    bound: &'static str,
    datetime_value: Option<DateTime<Utc>>,
    date_value: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    if datetime_value.is_some() && date_value.is_some() {
        tracing::debug!(
            block = %block,
            bound,
            "block authors both the datetime and legacy date property; using the datetime one"
        );
    }
    datetime_value.or(date_value)
}

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

    /// Fail-fast startup check that the membership view exists (migration
    /// `0062_ranks_members_editors`). Without it the missing tables would only
    /// surface sporadically — on the first DAO-space recompute — instead of
    /// deterministically at boot with an explicit error. Deploy order matters:
    /// the migration must be applied before this indexer version starts.
    pub async fn check_membership_view(&self) -> Result<(), IndexerError> {
        for table in ["ranks.members", "ranks.editors"] {
            sqlx::query(&format!("SELECT 1 FROM {table} LIMIT 1"))
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    IndexerError::Config(format!(
                        "{table} not readable — has migration 0062_ranks_members_editors \
                         been applied? ({e})"
                    ))
                })?;
        }
        Ok(())
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
            "SELECT id, space_id, name, filter, start_date, end_date, restriction_id, \
                    ranking_type, submission_frequency \
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

        // Config properties: Name/Filter as text, Start/End as datetime (both
        // the legacy date property and its GEO-2253 datetime successor — see
        // `resolve_window_bound`), Submission frequency (GEO-2328) as integer
        // hours. One row is always returned (aggregates over zero matching
        // values yield NULLs).
        type ConfigRow = (
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
            Option<i64>,
        );
        // Scope to the block's home space: the same entity may be perspectived
        // into other spaces with different config, keyed on (id, space_id).
        let (
            name,
            filter,
            legacy_start_date,
            legacy_end_date,
            start_datetime,
            end_datetime,
            submission_frequency,
        ): ConfigRow = sqlx::query_as(
            "SELECT \
                   max(text)         FILTER (WHERE property_id = $2) AS name, \
                   max(text)         FILTER (WHERE property_id = $3) AS filter, \
                   max(datetime_utc) FILTER (WHERE property_id = $4) AS start_date, \
                   max(datetime_utc) FILTER (WHERE property_id = $5) AS end_date, \
                   max(datetime_utc) FILTER (WHERE property_id = $6) AS start_datetime, \
                   max(datetime_utc) FILTER (WHERE property_id = $7) AS end_datetime, \
                   max(integer)      FILTER (WHERE property_id = $8) AS submission_frequency \
                 FROM values WHERE entity_id = $1 AND space_id = $9",
        )
        .bind(block_id)
        .bind(pid(ids::NAME_PROPERTY_ID))
        .bind(pid(ids::RANK_FILTER_PROPERTY_ID))
        .bind(pid(ids::RANK_START_DATE_PROPERTY_ID))
        .bind(pid(ids::RANK_END_DATE_PROPERTY_ID))
        .bind(pid(ids::RANK_START_DATETIME_PROPERTY_ID))
        .bind(pid(ids::RANK_END_DATETIME_PROPERTY_ID))
        .bind(pid(ids::RANK_SUBMISSION_FREQUENCY_PROPERTY_ID))
        .bind(space_id)
        .fetch_one(&self.pool)
        .await?;

        // The datetime property (GEO-2253) wins per-bound; the legacy date
        // property is the fallback — same precedence as `detect::window_bound`,
        // so the live and KG-recovery paths never disagree on a block that
        // authors both.
        let start_date = resolve_window_bound(block_id, "start", start_datetime, legacy_start_date);
        let end_date = resolve_window_bound(block_id, "end", end_datetime, legacy_end_date);

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

        // Rolling is a second `TYPES` relation on the block (GEO-2328) — same
        // relation type as the `Ranking Block` typing above, different target
        // — not a dedicated property. See RANK_ROLLING_TYPE_ID's doc comment.
        let (is_rolling,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 AND to_entity_id = $3 AND space_id = $4)",
        )
        .bind(block_id)
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANK_ROLLING_TYPE_ID))
        .bind(space_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(Some(RankingBlock {
            id: block_id,
            space_id,
            name,
            filter,
            start_date,
            end_date,
            restriction_id: restriction.map(|(r,)| r),
            ranking_type: is_rolling.then(|| pid(ids::RANK_ROLLING_TYPE_ID)),
            submission_frequency: submission_frequency.and_then(|v| i32::try_from(v).ok()),
        }))
    }

    /// Entities typed `Ranking Block` in the indexed public graph that never
    /// made it into `ranks.ranking_blocks`.
    ///
    /// `get_block_config_from_kg` (issue #738/#739) only ever runs when a rank
    /// happens to link to the block *after* the block's own typing/config is
    /// indexed; if no rank has linked to it since, or the link raced ahead of
    /// kg-indexer committing the type relation, the block is stuck here
    /// forever with nothing to retry it. Used by the one-off backfill binary
    /// (`bin/backfill_blocks.rs`) and can also back a periodic reconciliation
    /// sweep.
    pub async fn find_unregistered_ranking_blocks(&self) -> Result<Vec<Uuid>, IndexerError> {
        use sdk::core::ids;
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT r.from_entity_id FROM relations r \
             WHERE r.type_id = $1 AND r.to_entity_id = $2 \
               AND r.from_entity_id NOT IN (SELECT id FROM ranks.ranking_blocks)",
        )
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANKING_BLOCK_TYPE_ID))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Every already-registered Rolling block (GEO-2328) — `ranking_type` is
    /// `NOT NULL` — for the periodic sweep (`bin/rolling_sweep.rs`) to
    /// recompute. A Rolling block's aggregate can go stale purely from
    /// elapsed time (a submission ageing past `submission_frequency`), with
    /// no edit to react to, so unlike everything else in this indexer it
    /// needs a time-based trigger rather than an event-based one.
    pub async fn find_rolling_ranking_blocks(&self) -> Result<Vec<Uuid>, IndexerError> {
        let rows: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM ranks.ranking_blocks WHERE ranking_type IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// EVERY registered block, for a deliberate full recompute
    /// (`bin/backfill_blocks.rs --recompute-all`).
    ///
    /// Every other trigger in this indexer is reactive — an edit, a membership
    /// event, a rank appearing, time elapsing for a Rolling block. None of them
    /// fire when the *inputs* an aggregate was derived from are corrected
    /// underneath it by something outside the indexer.
    ///
    /// That gap is not hypothetical: on 2026-07-31 every migrated
    /// `ranks.rankings.submitted_at` was repaired (the chain migration had
    /// stamped 349 submissions with migration wall-clock time instead of their
    /// real dates, collapsing 348 distinct timestamps into 101). Eligibility is
    /// decided by `submitted_at` against a block's date window, so the stored
    /// aggregates were computed from wrong inputs — and nothing recomputed them,
    /// because no edit had happened. `backfill_blocks` reported
    /// `blocks=0 stale_blocks=0`: correct for what it looks at, useless here.
    pub async fn find_all_ranking_blocks(&self) -> Result<Vec<Uuid>, IndexerError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM ranks.ranking_blocks ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Already-registered blocks (`ranks.ranking_blocks` row exists) whose
    /// `ranking_type` is still `NULL` even though the graph now tags them
    /// Rolling — the same split-edit gap as `find_unregistered_ranking_blocks`,
    /// one step later: `detect()` only sets `ranking_type` from a
    /// `CreateEntity`'s own ops, so a block created as static and tagged
    /// Rolling by a *later*, separate edit never gets its row updated (issue
    /// found investigating GEO-2328/PR#821 — a real block on a live space sat
    /// stuck this way until backfilled by hand). Used by `bin/backfill_blocks.rs`
    /// alongside `find_unregistered_ranking_blocks`; the recovery step is
    /// identical (`get_block_config_from_kg` + `upsert_ranking_block`), just
    /// starting from a row that already exists instead of one that doesn't.
    pub async fn find_stale_ranking_type_blocks(&self) -> Result<Vec<Uuid>, IndexerError> {
        use sdk::core::ids;
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT rb.id FROM ranks.ranking_blocks rb \
             WHERE rb.ranking_type IS NULL \
               AND EXISTS ( \
                 SELECT 1 FROM relations r \
                 WHERE r.from_entity_id = rb.id AND r.type_id = $1 AND r.to_entity_id = $2 \
               )",
        )
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANK_ROLLING_TYPE_ID))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Entities typed `Rank` in the indexed public graph that never made it
    /// into `ranks.rankings` — the identical split-edit gap as
    /// `find_unregistered_ranking_blocks`, but for Rank submissions.
    ///
    /// Unlike blocks, nothing in the live consumer ever retries this (no
    /// `get_block_config_from_kg`-style reactive trigger exists for ranks),
    /// so recovery here is driven entirely by the periodic sweep — see
    /// `get_rank_config_from_kg`.
    pub async fn find_unregistered_ranks(&self) -> Result<Vec<Uuid>, IndexerError> {
        use sdk::core::ids;
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT DISTINCT r.from_entity_id FROM relations r \
             WHERE r.type_id = $1 AND r.to_entity_id = $2 \
               AND r.from_entity_id NOT IN (SELECT id FROM ranks.rankings)",
        )
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANK_TYPE_ID))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Reconstruct a Rank submission's config from the indexed public graph.
    ///
    /// Mirrors `get_block_config_from_kg` (issue #738/#739) for the identical
    /// gap on Rank entities: `detect()` only registers a rank when its
    /// `TYPES -> Rank` relation and its `rank_type` property arrive in the
    /// *same* edit. There is deliberately no reactive live-path trigger for
    /// this (unlike blocks, which recover the moment a rank links to one) —
    /// recovery is periodic-sweep-only, so this also resolves `block_id`
    /// directly from the `RANK_BLOCK` relation here rather than depending on
    /// `set_ranking_block`'s live path, which is a bare `UPDATE` that
    /// silently touches zero rows against a not-yet-registered rank.
    ///
    /// `submitted_at`/`updated_at_block` come from `public.entities`
    /// (`updated_at`/`updated_at_block`, not `created_at`, so dedup ordering
    /// reflects the *latest* on-chain change) rather than a live edit's
    /// metadata, since a recovery run has no current edit to take them from.
    /// `update_index` has no public-schema equivalent; recovered ranks get
    /// `0`, which only risks a wrong dedup tie-break against another
    /// submission landing in the exact same chain block (narrow, accepted).
    /// `author_address` is `None`, matching what the live path always
    /// produces today (unimplemented there too — not a recovery gap).
    /// Returns `None` if the entity is not (yet) typed as a Rank.
    pub async fn get_rank_config_from_kg(
        &self,
        rank_id: Uuid,
    ) -> Result<Option<Ranking>, IndexerError> {
        use sdk::core::ids;
        let pid = |s: &str| Uuid::parse_str(s).expect("valid system ID constant");

        let typed: Option<(Uuid,)> = sqlx::query_as(
            "SELECT space_id FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 AND to_entity_id = $3 \
             LIMIT 1",
        )
        .bind(rank_id)
        .bind(pid(ids::TYPE_RELATION_TYPE_ID))
        .bind(pid(ids::RANK_TYPE_ID))
        .fetch_optional(&self.pool)
        .await?;
        let Some((space_id,)) = typed else {
            return Ok(None);
        };

        let rank_type: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT text FROM values \
             WHERE entity_id = $1 AND space_id = $2 AND property_id = $3 LIMIT 1",
        )
        .bind(rank_id)
        .bind(space_id)
        .bind(pid(ids::RANK_TYPE_PROPERTY_ID))
        .fetch_optional(&self.pool)
        .await?;
        let rank_type = rank_type.and_then(|(t,)| t);

        let block_id: Option<(Uuid,)> = sqlx::query_as(
            "SELECT to_entity_id FROM relations \
             WHERE from_entity_id = $1 AND type_id = $2 LIMIT 1",
        )
        .bind(rank_id)
        .bind(pid(ids::RANK_BLOCK_RELATION_TYPE_ID))
        .fetch_optional(&self.pool)
        .await?;

        let entity: Option<(String, String)> =
            sqlx::query_as("SELECT updated_at, updated_at_block FROM entities WHERE id = $1")
                .bind(rank_id)
                .fetch_optional(&self.pool)
                .await?;
        let (submitted_at, updated_at_block) = match entity {
            Some((updated_at, updated_at_block)) => (
                updated_at
                    .parse::<i64>()
                    .ok()
                    .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
                updated_at_block.parse::<i64>().unwrap_or(0),
            ),
            None => (None, 0),
        };

        Ok(Some(Ranking {
            id: rank_id,
            block_id: block_id.map(|(b,)| b),
            space_id,
            author_address: None,
            rank_type,
            submitted_at,
            updated_at_block,
            update_index: 0,
        }))
    }

    /// The chain height/timestamp hermes has indexed up to, for stamping
    /// entities the backfill binary mints (it runs off-band, with no edit of
    /// its own to take `BlockMeta` from). Falls back to `(0, 0)` if the cursor
    /// row is absent (e.g. a fresh/local database) — the projection is stamped
    /// but otherwise unaffected, since dedup/ordering key off `rankings`'
    /// own `updated_at_block`, not this value.
    pub async fn current_chain_meta(&self) -> Result<BlockMeta, IndexerError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT block_number FROM meta WHERE id = 'hermes_pipeline'")
                .fetch_optional(&self.pool)
                .await?;
        let number = row.and_then(|(s,)| s.parse::<i64>().ok()).unwrap_or(0);
        Ok(BlockMeta {
            number,
            timestamp: Utc::now().timestamp(),
        })
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
                (id, space_id, name, filter, start_date, end_date, restriction_id,
                 ranking_type, submission_frequency, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
            ON CONFLICT (id, space_id) DO UPDATE SET
                name = EXCLUDED.name,
                filter = EXCLUDED.filter,
                start_date = EXCLUDED.start_date,
                end_date = EXCLUDED.end_date,
                restriction_id = EXCLUDED.restriction_id,
                ranking_type = EXCLUDED.ranking_type,
                submission_frequency = EXCLUDED.submission_frequency,
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
        .bind(b.ranking_type)
        .bind(b.submission_frequency)
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
        meta: BlockMeta,
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

        // 2. Drop prior projection relations (RANK_POSITION + Aggregated rankings)
        //    that are no longer part of the recomputed projection. Scoped by
        //    (from_entity_id, type_id, space_id) only — NOT by from_space_id,
        //    so pre-fix rows written with from_space_id IS NULL are still
        //    cleared (NULL never equals block_space_id). The INSERTs below are
        //    idempotent (ON CONFLICT on the PK), so even a row this DELETE
        //    misses converges on re-insert instead of raising a duplicate-key
        //    error — a unique violation here is also reclassified as poison
        //    (see error.rs), so it can never crash-loop the partition again.
        sqlx::query(
            "DELETE FROM relations WHERE from_entity_id = $1 AND type_id = ANY($2) AND space_id = $3",
        )
        .bind(block_id)
        .bind(&[rank_position, aggregated][..])
        .bind(block_space_id)
        .execute(&mut *tx)
        .await?;

        // 3. Register every entity this projection mints in `entities` (the
        //    source of truth for entity existence the API resolves against).
        let mut entity_ids: Vec<Uuid> =
            Vec::with_capacity(rows.len() * 2 + contributing_rankings.len() * 2);
        for r in rows {
            entity_ids.push(r.relation_id);
            entity_ids.push(r.reified_entity_id);
        }
        for ranking_id in contributing_rankings {
            let (relation_id, reified) = provenance_ids(block_id, *ranking_id);
            entity_ids.push(relation_id);
            entity_ids.push(reified);
        }
        if !entity_ids.is_empty() {
            // `entities` columns are text: Unix seconds + block number, as the
            // kg-indexer records. `created_at` only sticks on first insert;
            // re-aggregation just bumps `updated_at`.
            let created_at = meta.timestamp.to_string();
            let created_at_block = meta.number.to_string();
            let stamps = vec![created_at; entity_ids.len()];
            let blocks = vec![created_at_block; entity_ids.len()];
            sqlx::query(
                "INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block) \
                 SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $2::text[], $3::text[]) \
                 ON CONFLICT (id) DO UPDATE SET \
                   updated_at = EXCLUDED.updated_at, updated_at_block = EXCLUDED.updated_at_block",
            )
            .bind(&entity_ids)
            .bind(&stamps)
            .bind(&blocks)
            .execute(&mut *tx)
            .await?;
        }

        // 4. Insert the ordered RANK_POSITION relations + their value rows.
        for r in rows {
            sqlx::query(
                "INSERT INTO relations \
                 (id, entity_id, type_id, from_entity_id, from_space_id, to_entity_id, to_space_id, space_id, position) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                 ON CONFLICT (id) DO UPDATE SET \
                   entity_id = EXCLUDED.entity_id, \
                   to_entity_id = EXCLUDED.to_entity_id, \
                   to_space_id = EXCLUDED.to_space_id, \
                   position = EXCLUDED.position",
            )
            .bind(r.relation_id)
            .bind(r.reified_entity_id)
            .bind(rank_position)
            .bind(block_id)
            .bind(block_space_id)
            .bind(r.entity_id)
            .bind(r.space_id)
            .bind(block_space_id)
            .bind(&r.position)
            .execute(&mut *tx)
            .await?;

            // `values.id` is a text column — serialize the UUID at the bind boundary.
            sqlx::query(
                "INSERT INTO values (id, entity_id, space_id, property_id, integer) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (id) DO UPDATE SET \
                   entity_id = EXCLUDED.entity_id, \
                   space_id = EXCLUDED.space_id, \
                   property_id = EXCLUDED.property_id, \
                   integer = EXCLUDED.integer",
            )
            .bind(r.value_row_id.to_string())
            .bind(r.reified_entity_id)
            .bind(block_space_id)
            .bind(value_prop)
            .bind(r.value)
            .execute(&mut *tx)
            .await?;
        }

        // 5. Insert Aggregated rankings provenance relations (block -> submission).
        for ranking_id in contributing_rankings {
            let (relation_id, reified) = provenance_ids(block_id, *ranking_id);
            // Provenance is a deterministic block -> submission mapping; on a
            // recompute the same ids re-resolve to the same row, so a conflict
            // just means it's already present.
            sqlx::query(
                "INSERT INTO relations \
                 (id, entity_id, type_id, from_entity_id, from_space_id, to_entity_id, space_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(relation_id)
            .bind(reified)
            .bind(aggregated)
            .bind(block_id)
            .bind(block_space_id)
            .bind(ranking_id)
            .bind(block_space_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
