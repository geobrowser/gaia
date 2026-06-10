//! Scoring: turn eligible submissions into a single ordered aggregate.
//!
//! Per the team's decisions:
//!   - Each ballot's item values are normalized to `[NORM_LO, NORM_HI]` = `[0.5, 1.0]`
//!     by default, so being ranked at all is positive signal (floor 0.5) and
//!     position scales up to 1.0.
//!   - WEIGHTED ballots: min-max normalize the provided weights into the range.
//!   - ORDINAL ballots: linear map by position (best = 1.0, worst = 0.5).
//!   - Mixing ordinal + weighted in one block is fine — both land in the same range.
//!
//! Normalized contributions are summed per `(entity, space)` across ballots,
//! sorted descending (tie-broken deterministically), and assigned integer
//! positions. The summed `score` stays continuous (`f64`) — the integer
//! projection for publishing happens later.

use std::cmp::Ordering;
use std::collections::HashMap;

use uuid::Uuid;

use crate::models::{Ranking, RankingItem};

/// Default normalization floor: a ranked item always carries positive signal.
pub const NORM_LO: f64 = 0.5;
/// Default normalization ceiling: the top of a ballot.
pub const NORM_HI: f64 = 1.0;

/// A computed aggregate row (mirrors `ranks.ranking_scores`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreRow {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub score: f64,
    /// 1-based rank within the block.
    pub position: i32,
}

fn is_weighted(rank_type: Option<&str>) -> bool {
    matches!(rank_type, Some(t) if t.eq_ignore_ascii_case("WEIGHTED"))
}

/// Normalize one ballot's items to `[lo, hi]`, returning `(entity, space) -> value`.
pub fn normalize_ballot(
    ranking: &Ranking,
    items: &[RankingItem],
    lo: f64,
    hi: f64,
) -> Vec<((Uuid, Uuid), f64)> {
    if items.is_empty() {
        return Vec::new();
    }
    if is_weighted(ranking.rank_type.as_deref()) {
        normalize_weighted(items, lo, hi)
    } else {
        normalize_ordinal(items, lo, hi)
    }
}

/// WEIGHTED: min-max the provided weights into `[lo, hi]`. Items without a
/// weight can't be scored on a weighted ballot and are skipped. A degenerate
/// range (single item / all-equal) maps everything to `hi`.
fn normalize_weighted(items: &[RankingItem], lo: f64, hi: f64) -> Vec<((Uuid, Uuid), f64)> {
    let weighted: Vec<&RankingItem> = items.iter().filter(|i| i.weight.is_some()).collect();
    if weighted.is_empty() {
        return Vec::new();
    }
    let min = weighted
        .iter()
        .map(|i| i.weight.unwrap())
        .fold(f64::INFINITY, f64::min);
    let max = weighted
        .iter()
        .map(|i| i.weight.unwrap())
        .fold(f64::NEG_INFINITY, f64::max);

    weighted
        .iter()
        .map(|i| {
            let w = i.weight.unwrap();
            let norm = if max > min {
                lo + (hi - lo) * (w - min) / (max - min)
            } else {
                hi
            };
            ((i.entity_id, i.space_id), norm)
        })
        .collect()
}

/// ORDINAL: order items by fractional index (ascending = best-first) and map
/// linearly into `[lo, hi]` (best = hi, worst = lo). A single item maps to `hi`.
fn normalize_ordinal(items: &[RankingItem], lo: f64, hi: f64) -> Vec<((Uuid, Uuid), f64)> {
    let mut ordered: Vec<&RankingItem> = items.iter().collect();
    // Sort by fractional index; items missing a position sort last (deterministic).
    ordered.sort_by(|a, b| {
        (a.position.is_none(), &a.position).cmp(&(b.position.is_none(), &b.position))
    });

    let n = ordered.len();
    ordered
        .iter()
        .enumerate()
        .map(|(rank, item)| {
            let value = if n > 1 {
                hi - (hi - lo) * (rank as f64) / ((n - 1) as f64)
            } else {
                hi
            };
            ((item.entity_id, item.space_id), value)
        })
        .collect()
}

/// Aggregate eligible ballots into the block's ordered result.
pub fn aggregate(ballots: &[(&Ranking, Vec<RankingItem>)], lo: f64, hi: f64) -> Vec<ScoreRow> {
    let mut totals: HashMap<(Uuid, Uuid), f64> = HashMap::new();
    for (ranking, items) in ballots {
        for (key, value) in normalize_ballot(ranking, items, lo, hi) {
            *totals.entry(key).or_insert(0.0) += value;
        }
    }

    let mut rows: Vec<((Uuid, Uuid), f64)> = totals.into_iter().collect();
    // Descending score; deterministic tie-break by (entity, space).
    rows.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0 .0.cmp(&b.0 .0))
            .then_with(|| a.0 .1.cmp(&b.0 .1))
    });

    rows.into_iter()
        .enumerate()
        .map(|(i, ((entity_id, space_id), score))| ScoreRow {
            entity_id,
            space_id,
            score,
            position: (i as i32) + 1,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranking(id: u128, rank_type: &str) -> Ranking {
        Ranking {
            id: Uuid::from_u128(id),
            block_id: Some(Uuid::from_u128(1)),
            space_id: Uuid::from_u128(900),
            author_address: None,
            rank_type: Some(rank_type.to_string()),
            submitted_at: None,
            updated_at_block: 0,
            update_index: 0,
        }
    }

    fn item(entity: u128, position: Option<&str>, weight: Option<f64>) -> RankingItem {
        RankingItem {
            ranking_id: Uuid::from_u128(0),
            entity_id: Uuid::from_u128(entity),
            space_id: Uuid::from_u128(500),
            position: position.map(|s| s.to_string()),
            weight,
        }
    }

    fn value_for(out: &[((Uuid, Uuid), f64)], entity: u128) -> f64 {
        out.iter()
            .find(|((e, _), _)| *e == Uuid::from_u128(entity))
            .map(|(_, v)| *v)
            .expect("entity present")
    }

    #[test]
    fn ordinal_maps_linearly_into_floor_to_ceiling() {
        let r = ranking(1, "ORDINAL");
        let items = vec![
            item(10, Some("a0"), None),
            item(11, Some("a1"), None),
            item(12, Some("a2"), None),
        ];
        let out = normalize_ballot(&r, &items, NORM_LO, NORM_HI);
        assert_eq!(value_for(&out, 10), 1.0);
        assert_eq!(value_for(&out, 11), 0.75);
        assert_eq!(value_for(&out, 12), 0.5);
    }

    #[test]
    fn weighted_min_max_into_floor_to_ceiling() {
        let r = ranking(1, "WEIGHTED");
        let items = vec![
            item(10, Some("a0"), Some(90.0)),
            item(11, Some("a1"), Some(65.0)),
            item(12, Some("a2"), Some(50.0)),
        ];
        let out = normalize_ballot(&r, &items, NORM_LO, NORM_HI);
        assert_eq!(value_for(&out, 10), 1.0); // max -> hi
        assert_eq!(value_for(&out, 12), 0.5); // min -> lo
                                              // 65 is 15/40 of the way: 0.5 + 0.5*0.375 = 0.6875
        assert!((value_for(&out, 11) - 0.6875).abs() < 1e-9);
    }

    #[test]
    fn single_item_maps_to_ceiling() {
        let ord = ranking(1, "ORDINAL");
        let out = normalize_ballot(&ord, &[item(10, Some("a0"), None)], NORM_LO, NORM_HI);
        assert_eq!(value_for(&out, 10), 1.0);

        let wtd = ranking(2, "WEIGHTED");
        let out = normalize_ballot(&wtd, &[item(10, Some("a0"), Some(7.0))], NORM_LO, NORM_HI);
        assert_eq!(value_for(&out, 10), 1.0);
    }

    #[test]
    fn all_equal_weights_map_to_ceiling() {
        let r = ranking(1, "WEIGHTED");
        let items = vec![item(10, None, Some(3.0)), item(11, None, Some(3.0))];
        let out = normalize_ballot(&r, &items, NORM_LO, NORM_HI);
        assert_eq!(value_for(&out, 10), 1.0);
        assert_eq!(value_for(&out, 11), 1.0);
    }

    #[test]
    fn aggregate_sums_sorts_and_positions() {
        // Two ordinal ballots agree A > B; a third ranks B first.
        let r1 = ranking(1, "ORDINAL");
        let b1 = vec![item(100, Some("a0"), None), item(200, Some("a1"), None)]; // A=1.0, B=0.5
        let r2 = ranking(2, "ORDINAL");
        let b2 = vec![item(100, Some("a0"), None), item(200, Some("a1"), None)]; // A=1.0, B=0.5
        let r3 = ranking(3, "ORDINAL");
        let b3 = vec![item(200, Some("a0"), None), item(100, Some("a1"), None)]; // B=1.0, A=0.5

        let ballots = vec![(&r1, b1), (&r2, b2), (&r3, b3)];
        let rows = aggregate(&ballots, NORM_LO, NORM_HI);

        // A: 1.0 + 1.0 + 0.5 = 2.5 ; B: 0.5 + 0.5 + 1.0 = 2.0
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].entity_id, Uuid::from_u128(100));
        assert_eq!(rows[0].position, 1);
        assert!((rows[0].score - 2.5).abs() < 1e-9);
        assert_eq!(rows[1].entity_id, Uuid::from_u128(200));
        assert_eq!(rows[1].position, 2);
        assert!((rows[1].score - 2.0).abs() < 1e-9);
    }

    #[test]
    fn aggregate_keys_on_entity_and_space() {
        // Same entity under two different space perspectives stays distinct.
        let r = ranking(1, "ORDINAL");
        let items = vec![
            RankingItem {
                ranking_id: Uuid::from_u128(1),
                entity_id: Uuid::from_u128(100),
                space_id: Uuid::from_u128(1),
                position: Some("a0".into()),
                weight: None,
            },
            RankingItem {
                ranking_id: Uuid::from_u128(1),
                entity_id: Uuid::from_u128(100),
                space_id: Uuid::from_u128(2),
                position: Some("a1".into()),
                weight: None,
            },
        ];
        let rows = aggregate(&[(&r, items)], NORM_LO, NORM_HI);
        assert_eq!(rows.len(), 2); // two perspectives -> two rows
    }
}
