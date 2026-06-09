//! Row models mirroring the private `ranks` working schema.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A `ranks.ranking_blocks` row — one per Ranking Block entity.
#[derive(Debug, Clone)]
pub struct RankingBlock {
    pub id: Uuid,
    pub space_id: Uuid,
    pub name: Option<String>,
    pub filter: Option<String>,
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    /// Aggregation restriction value entity; `None` => default "Members and editors".
    pub restriction_id: Option<Uuid>,
}

/// A `ranks.rankings` row — one per Rank submission.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct RankingItem {
    pub ranking_id: Uuid,
    pub entity_id: Uuid,
    pub space_id: Uuid,
    /// Fractional index for ordinal ordering.
    pub position: Option<String>,
    /// Weighted value (`None` for ordinal ranks).
    pub weight: Option<f64>,
}
