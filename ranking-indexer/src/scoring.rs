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

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{Ranking, RankingItem};

/// Default normalization floor: a ranked item always carries positive signal.
pub const NORM_LO: f64 = 0.5;
/// Default normalization ceiling: the top of a ballot.
pub const NORM_HI: f64 = 1.0;

/// How many half-lives a Rolling ballot keeps contributing before it is dropped
/// outright (see `eligibility::rolling_admits`).
///
/// Decay alone never reaches zero, so without a cutoff every entity ever ranked
/// would stay in the projection forever carrying a vanishing score, and the
/// published table would grow without bound. At eight half-lives a ballot is
/// down to 1/256 of its original weight — far below the resolution of the
/// integer 0–100 projection — so dropping it there is invisible in the output
/// while keeping the table bounded.
pub const ROLLING_MAX_HALF_LIVES: f64 = 8.0;

/// Recency decay for a Rolling block: ballots lose half their weight every
/// `half_life_hours`.
///
/// This replaces the hard 24h cliff that `submission_frequency` used to impose.
/// The cliff meant a block published *nothing* whenever no one had submitted
/// inside the window — routinely, since the window equalled the resubmission
/// cadence — so a "Trending" table read as empty rather than stale. Decaying
/// instead keeps fresh ballots dominant without the table ever emptying.
#[derive(Debug, Clone, Copy)]
pub struct RecencyDecay {
    /// Age at which a ballot's contribution halves — the block's
    /// `submission_frequency`, reinterpreted as a half-life rather than a cliff.
    pub half_life_hours: f64,
    /// Instant ages are measured against. A parameter, never an inline
    /// `Utc::now()`, so a sweep and a test can both pin it.
    pub now: DateTime<Utc>,
}

/// A ballot's recency multiplier: `0.5 ^ (age / half_life)`, clamped to `[0, 1]`.
///
/// A ballot with no `submitted_at`, or a non-positive half-life, can't be aged
/// and so decays not at all — consistent with `eligibility`, which admits rather
/// than silently drops what it cannot evaluate. Ballots dated in the future
/// (clock skew between the chain and this process) are treated as brand new
/// rather than amplified beyond full weight.
pub fn recency_weight(decay: RecencyDecay, submitted_at: Option<DateTime<Utc>>) -> f64 {
    let Some(submitted_at) = submitted_at else {
        return 1.0;
    };
    if !decay.half_life_hours.is_finite() || decay.half_life_hours <= 0.0 {
        return 1.0;
    }
    let age_hours = (decay.now - submitted_at).num_milliseconds() as f64 / 3_600_000.0;
    if age_hours <= 0.0 {
        return 1.0;
    }
    0.5_f64.powf(age_hours / decay.half_life_hours)
}

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
///
/// `decay` is `Some` only for Rolling blocks, where each ballot's contribution is
/// scaled by its recency (see [`recency_weight`]). Static blocks pass `None` and
/// are scored exactly as before — every eligible ballot at full weight.
pub fn aggregate(
    ballots: &[(&Ranking, Vec<RankingItem>)],
    lo: f64,
    hi: f64,
    decay: Option<RecencyDecay>,
) -> Vec<ScoreRow> {
    let mut totals: HashMap<(Uuid, Uuid), f64> = HashMap::new();
    for (ranking, items) in ballots {
        let weight = match decay {
            Some(decay) => recency_weight(decay, ranking.submitted_at),
            None => 1.0,
        };
        for (key, value) in normalize_ballot(ranking, items, lo, hi) {
            *totals.entry(key).or_insert(0.0) += value * weight;
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
        let rows = aggregate(&ballots, NORM_LO, NORM_HI, None);

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
        let rows = aggregate(&[(&r, items)], NORM_LO, NORM_HI, None);
        assert_eq!(rows.len(), 2); // two perspectives -> two rows
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap()
    }

    fn ranking_submitted(id: u128, rank_type: &str, submitted_at: DateTime<Utc>) -> Ranking {
        Ranking {
            submitted_at: Some(submitted_at),
            ..ranking(id, rank_type)
        }
    }

    #[test]
    fn recency_weight_halves_every_half_life() {
        let hour = 3600;
        let decay = |now: i64| RecencyDecay {
            half_life_hours: 24.0,
            now: at(now),
        };
        let submitted = Some(at(0));
        assert!((recency_weight(decay(0), submitted) - 1.0).abs() < 1e-12);
        assert!((recency_weight(decay(24 * hour), submitted) - 0.5).abs() < 1e-12);
        assert!((recency_weight(decay(48 * hour), submitted) - 0.25).abs() < 1e-12);
        // Never reaches zero — that is what keeps the table from emptying.
        assert!(recency_weight(decay(1000 * hour), submitted) > 0.0);
    }

    #[test]
    fn recency_weight_admits_what_it_cannot_age() {
        let hour = 3600;
        let decay = RecencyDecay {
            half_life_hours: 24.0,
            now: at(100 * hour),
        };
        // Unknown submission time -> full weight, not silently zeroed.
        assert_eq!(recency_weight(decay, None), 1.0);
        // Non-positive half-life can't decay anything.
        assert_eq!(
            recency_weight(
                RecencyDecay {
                    half_life_hours: 0.0,
                    ..decay
                },
                Some(at(0))
            ),
            1.0
        );
        // Future-dated (clock skew) is capped at full weight, never amplified.
        assert_eq!(recency_weight(decay, Some(at(200 * hour))), 1.0);
    }

    #[test]
    fn decay_lets_a_fresh_ballot_outrank_an_older_agreeing_majority() {
        let hour = 3600;
        // Two stale ballots rank A first; one fresh ballot ranks B first.
        let stale_a = ranking_submitted(1, "ORDINAL", at(0));
        let stale_b = ranking_submitted(2, "ORDINAL", at(0));
        let fresh = ranking_submitted(3, "ORDINAL", at(96 * hour));
        let a_first = || vec![item(100, Some("a0"), None), item(200, Some("a1"), None)];
        let b_first = vec![item(200, Some("a0"), None), item(100, Some("a1"), None)];

        let ballots = vec![
            (&stale_a, a_first()),
            (&stale_b, a_first()),
            (&fresh, b_first),
        ];

        // Undecayed, the two agreeing ballots win: A = 2.5, B = 2.0.
        let flat = aggregate(&ballots, NORM_LO, NORM_HI, None);
        assert_eq!(flat[0].entity_id, Uuid::from_u128(100));

        // With a 24h half-life the 4-day-old ballots are down to 1/16, so the
        // fresh ballot's preference leads.
        let decayed = aggregate(
            &ballots,
            NORM_LO,
            NORM_HI,
            Some(RecencyDecay {
                half_life_hours: 24.0,
                now: at(96 * hour),
            }),
        );
        assert_eq!(decayed[0].entity_id, Uuid::from_u128(200));
        assert_eq!(decayed[0].position, 1);
        // Stale ballots still contribute — they are decayed, not discarded.
        assert!(decayed[1].score > 0.0);
    }

    #[test]
    fn decay_never_empties_a_block_that_has_ballots() {
        let hour = 3600;
        let old = ranking_submitted(1, "ORDINAL", at(0));
        let ballots = vec![(&old, vec![item(100, Some("a0"), None)])];
        let rows = aggregate(
            &ballots,
            NORM_LO,
            NORM_HI,
            Some(RecencyDecay {
                half_life_hours: 24.0,
                now: at(120 * hour), // 5 days old
            }),
        );
        assert_eq!(rows.len(), 1);
        assert!(rows[0].score > 0.0);
    }
}
