//! Row models mirroring the private `ranks` working schema.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Block provenance for entities the projection mints, carried from the
/// triggering edit or membership event down to the public-graph write. The
/// `entities` table requires `created_at`/`*_block` (Unix seconds + block
/// number as text), matching what the kg-indexer records.
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockMeta {
    pub number: i64,
    pub timestamp: i64,
}

/// A `ranks.ranking_blocks` row — one per Ranking Block entity.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RankingBlock {
    pub id: Uuid,
    pub space_id: Uuid,
    pub name: Option<String>,
    pub filter: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    /// Aggregation restriction value entity; `None` => default "Members and editors".
    pub restriction_id: Option<Uuid>,
    /// `Ranking type` value entity (GEO-2328), e.g. `RANK_ROLLING_TYPE_VALUE_ID`;
    /// `None` => default (static, the only kind that existed before GEO-2328).
    pub ranking_type: Option<Uuid>,
    /// Hours a submission stays eligible after `submitted_at` before requiring
    /// resubmission. Only meaningful when `ranking_type` is Rolling; `None`
    /// otherwise (static blocks use the block-wide `[start_date, end_date]`
    /// window instead — see `eligibility::filter_eligible`).
    pub submission_frequency: Option<i32>,
}

/// A `ranks.rankings` row — one per Rank submission.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Ranking {
    pub id: Uuid,
    /// `None` until the `RANK_BLOCK` link arrives (partial-state model).
    pub block_id: Option<Uuid>,
    pub space_id: Uuid,
    pub author_address: Option<String>,
    /// "ORDINAL" | "WEIGHTED".
    pub rank_type: Option<String>,
    pub submitted_at: Option<DateTime<Utc>>,
    /// Update markers for dedup: most-recently-updated rank per (block, space) wins.
    pub updated_at_block: i64,
    pub update_index: i64,
}

/// A `ranks.ranking_items` row — keyed on (ranking, entity, space).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RankingItem {
    pub ranking_id: Uuid,
    pub entity_id: Uuid,
    pub space_id: Uuid,
    /// Fractional index for ordinal ordering.
    pub position: Option<String>,
    /// Weighted value (`None` for ordinal ranks).
    pub weight: Option<f64>,
}
