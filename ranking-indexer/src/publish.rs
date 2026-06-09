//! Publish: project a block's computed aggregate into the public graph (design §8).
//!
//! Per ranked target we emit a `RANK_POSITION` relation (block -> ranked entity,
//! in the ranked perspective, ordered by `position`) whose reified entity carries
//! the integer rank-position value, plus an `Aggregated rankings` provenance
//! relation (block -> each contributing submission). IDs are deterministic from
//! `(block, entity, space)`, so re-aggregation upserts in place; the whole
//! projection for a block is replaced atomically each recompute.

use std::sync::LazyLock;

use uuid::Uuid;

use crate::scoring::ScoreRow;

static NS: LazyLock<Uuid> = LazyLock::new(|| {
    Uuid::parse_str(sdk::core::ids::GEO_SYSTEM_NAMESPACE).expect("valid GEO_SYSTEM_NAMESPACE")
});

fn derive(name: &str) -> Uuid {
    Uuid::new_v5(&NS, name.as_bytes())
}

/// One `RANK_POSITION` projection row for a ranked `(entity, space)`.
#[derive(Debug, Clone, PartialEq)]
pub struct RankPositionRow {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    /// Published integer score (block-scoped 0–100, v1).
    pub value: i64,
    /// Lexicographically-ordered position key.
    pub position: String,
    pub relation_id: Uuid,
    pub reified_entity_id: Uuid,
    pub value_row_id: Uuid,
}

/// Build the public projection from a block's scored aggregate.
///
/// Integer value is block-scoped (v1): `round(100 * score / max_score)`, so the
/// top entity is 100. Position is a fixed-width, lexicographically-ordered rank
/// key (regenerated each full recompute — fine since the projection is replaced
/// wholesale).
pub fn build_projection(block_id: Uuid, scores: &[ScoreRow]) -> Vec<RankPositionRow> {
    let max = scores.iter().map(|s| s.score).fold(0.0_f64, f64::max);
    scores
        .iter()
        .map(|s| {
            let value = if max > 0.0 {
                (100.0 * s.score / max).round() as i64
            } else {
                0
            };
            let tag = |kind: &str| derive(&format!("{kind}:{block_id}:{}:{}", s.entity_id, s.space_id));
            RankPositionRow {
                entity_id: s.entity_id,
                space_id: s.space_id,
                value,
                position: format!("{:010}", s.position),
                relation_id: tag("rank_position"),
                reified_entity_id: tag("rank_position_entity"),
                value_row_id: tag("rank_position_value"),
            }
        })
        .collect()
}

/// Deterministic `(relation_id, reified_entity_id)` for an `Aggregated rankings`
/// provenance relation linking a block to one contributing submission.
pub fn provenance_ids(block_id: Uuid, ranking_id: Uuid) -> (Uuid, Uuid) {
    (
        derive(&format!("aggregated_rankings:{block_id}:{ranking_id}")),
        derive(&format!("aggregated_rankings_entity:{block_id}:{ranking_id}")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(entity: u128, space: u128, score: f64, position: i32) -> ScoreRow {
        ScoreRow {
            entity_id: Uuid::from_u128(entity),
            space_id: Uuid::from_u128(space),
            score,
            position,
        }
    }

    #[test]
    fn scales_top_to_100_and_orders_by_position() {
        let block = Uuid::from_u128(1);
        let scores = vec![score(10, 1, 2.5, 1), score(11, 1, 2.0, 2), score(12, 1, 1.0, 3)];
        let proj = build_projection(block, &scores);
        assert_eq!(proj.len(), 3);
        assert_eq!(proj[0].value, 100); // 2.5/2.5
        assert_eq!(proj[1].value, 80); // 2.0/2.5
        assert_eq!(proj[2].value, 40); // 1.0/2.5
        assert!(proj[0].position < proj[1].position);
        assert!(proj[1].position < proj[2].position);
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let block = Uuid::from_u128(1);
        let s = vec![score(10, 1, 1.0, 1)];
        let a = build_projection(block, &s);
        let b = build_projection(block, &s);
        assert_eq!(a[0].relation_id, b[0].relation_id);
        assert_ne!(a[0].relation_id, a[0].reified_entity_id);
        assert_ne!(a[0].relation_id, a[0].value_row_id);
        assert_ne!(a[0].reified_entity_id, a[0].value_row_id);
    }

    #[test]
    fn empty_scores_yield_empty_projection() {
        assert!(build_projection(Uuid::from_u128(1), &[]).is_empty());
    }
}
