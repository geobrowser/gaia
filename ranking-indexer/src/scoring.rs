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
//!
//! A block can opt into a different shape through `ranks.ranking_block_config`
//! (GEO-3676, see [`BlockScoring`]): a higher row cap, and/or
//! [`ScoringMode::Average`], which scores every ballot by position and averages
//! per entity with a mid-list prior. Blocks with no config row are scored exactly
//! as described above.

use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::models::{Ranking, RankingItem};

/// Default normalization floor: a ranked item always carries positive signal.
pub const NORM_LO: f64 = 0.5;
/// Default normalization ceiling: the top of a ballot.
pub const NORM_HI: f64 = 1.0;

/// Most rows a block publishes. Everything below the cut is scored and stored;
/// only the projection is trimmed.
///
/// This replaces the age cutoff that used to bound the table (GEO-2869). That
/// cutoff dropped whole *ballots*, so an entity nothing recent had ranked left
/// the table entirely rather than sinking in it — a change of row set dressed
/// up as a change of order. Bounding by row count bounds the same thing without
/// that side effect: an old entity falls off only when enough better-scoring
/// ones arrive to push it past the cut, which is what a "trending" table should
/// do.
///
/// 200 is deliberately far above anything live. Measured across all 27 rolling
/// blocks on 2026-09-10, the largest table with no age cutoff at all would hold
/// **43** rows, so this bites nothing today and exists to keep a runaway block
/// bounded rather than to shape normal output.
pub const MAX_PROJECTION_ROWS: usize = 200;

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
    /// How many eligible ballots include this row (GEO-3676). Undecayed: a
    /// Rolling block's old ballot still counts as one.
    pub ballots: i32,
}

/// How a block combines its ballots (`ranks.ranking_block_config.scoring_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScoringMode {
    /// Sum each ballot's normalized value per `(entity, space)`. Every block
    /// without a config row.
    #[default]
    Sum,
    /// Average place per ballot, per entity (GEO-3676): see [`aggregate_with`].
    Average,
}

impl ScoringMode {
    /// The stored spelling, as constrained by the table's CHECK.
    pub fn as_str(self) -> &'static str {
        match self {
            ScoringMode::Sum => "sum",
            ScoringMode::Average => "average",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sum" => Some(ScoringMode::Sum),
            "average" => Some(ScoringMode::Average),
            _ => None,
        }
    }
}

/// A block's scoring shape: its row cap and how ballots combine. The default
/// (no `ranks.ranking_block_config` row) is what every block had before
/// GEO-3676: [`MAX_PROJECTION_ROWS`] rows, summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockScoring {
    pub max_rows: usize,
    pub mode: ScoringMode,
}

impl Default for BlockScoring {
    fn default() -> Self {
        BlockScoring {
            max_rows: MAX_PROJECTION_ROWS,
            mode: ScoringMode::Sum,
        }
    }
}

/// The prior an average-mode entity starts from: one phantom ballot placing it
/// mid-list (halfway between [`NORM_LO`] and [`NORM_HI`]). An entity placed
/// below the middle sinks with each such ballot and one placed above it rises,
/// and a single first place cannot outrank a debate many people placed high.
pub const AVERAGE_PRIOR_POINTS: f64 = 0.75;
/// How many phantom ballots the prior counts as.
pub const AVERAGE_PRIOR_BALLOTS: f64 = 1.0;

/// Average-mode scores are rounded to this many units per 1.0 before they are
/// compared or stored. Points like `1 - 0.5 * 3/4` are mostly not exact in
/// binary, so two equal averages can come out one ulp apart (0.675 against
/// 0.6749999999999999) and the "more ballots" tie-break would never run. A
/// nanoscore is far below any real difference between two debates' averages,
/// and far above the float error.
pub const AVERAGE_SCORE_UNITS: f64 = 1e9;

/// The integer an average-mode score is ordered by, and (divided back) stored as.
fn average_score_key(score: f64) -> i64 {
    (score * AVERAGE_SCORE_UNITS).round() as i64
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
///
/// Every entity on every eligible ballot gets a row, however old the ballot: a
/// decayed ballot sinks its entities toward the bottom, it does not delete
/// them. The result is capped at [`MAX_PROJECTION_ROWS`].
pub fn aggregate(
    ballots: &[(&Ranking, Vec<RankingItem>)],
    lo: f64,
    hi: f64,
    decay: Option<RecencyDecay>,
) -> Vec<ScoreRow> {
    aggregate_with(ballots, lo, hi, decay, BlockScoring::default())
}

/// [`aggregate`] under a block's own [`BlockScoring`].
///
/// [`ScoringMode::Sum`] is the original behaviour, capped at `config.max_rows`.
///
/// [`ScoringMode::Average`] (GEO-3676):
///   - every ballot is scored **by position** — #1 = `hi` down to last = `lo` —
///     whatever its rank type or weights (see [`ballot_points`]);
///   - rows are keyed by **entity only**: the same entity referenced under two
///     space ids is one row. Within one ballot it counts once, at its better
///     place. The row carries the space id the most ballots placed it under
///     (ties: the lowest space id), so the published `RANK_POSITION` relation
///     points at the perspective most voters used;
///   - `score = (Σ points + AVERAGE_PRIOR_POINTS) / (ballots + AVERAGE_PRIOR_BALLOTS)`.
///     On a Rolling block each ballot's points and its count are both scaled by
///     its recency, which reduces to the formula above with no decay;
///   - ties go to the entity on more ballots, then the lower entity id. Scores
///     are rounded to [`AVERAGE_SCORE_UNITS`] first, so averages that are equal
///     but differ by float error tie (and are stored as the same value).
pub fn aggregate_with(
    ballots: &[(&Ranking, Vec<RankingItem>)],
    lo: f64,
    hi: f64,
    decay: Option<RecencyDecay>,
    config: BlockScoring,
) -> Vec<ScoreRow> {
    let weight_of = |ranking: &Ranking| match decay {
        Some(decay) => recency_weight(decay, ranking.submitted_at),
        None => 1.0,
    };
    match config.mode {
        ScoringMode::Sum => {
            let mut totals: HashMap<(Uuid, Uuid), (f64, i32)> = HashMap::new();
            for (ranking, items) in ballots {
                let weight = weight_of(ranking);
                for (key, value) in normalize_ballot(ranking, items, lo, hi) {
                    let t = totals.entry(key).or_insert((0.0, 0));
                    t.0 += value * weight;
                    t.1 += 1;
                }
            }

            let mut rows: Vec<((Uuid, Uuid), (f64, i32))> = totals.into_iter().collect();
            // Descending score; deterministic tie-break by (entity, space).
            rows.sort_by(|a, b| {
                b.1 .0
                    .partial_cmp(&a.1 .0)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.0 .0.cmp(&b.0 .0))
                    .then_with(|| a.0 .1.cmp(&b.0 .1))
            });

            rows.into_iter()
                .take(config.max_rows)
                .enumerate()
                .map(|(i, ((entity_id, space_id), (score, ballots)))| ScoreRow {
                    entity_id,
                    space_id,
                    score,
                    position: (i as i32) + 1,
                    ballots,
                })
                .collect()
        }
        ScoringMode::Average => aggregate_average(ballots, lo, hi, weight_of, config.max_rows),
    }
}

#[derive(Default)]
struct AverageAcc {
    points: f64,
    weight: f64,
    ballots: i32,
    /// Ballots per space id this entity was placed under, to pick the row's space.
    spaces: HashMap<Uuid, i32>,
}

fn aggregate_average(
    ballots: &[(&Ranking, Vec<RankingItem>)],
    lo: f64,
    hi: f64,
    weight_of: impl Fn(&Ranking) -> f64,
    max_rows: usize,
) -> Vec<ScoreRow> {
    let mut acc: HashMap<Uuid, AverageAcc> = HashMap::new();
    for (ranking, items) in ballots {
        let weight = weight_of(ranking);
        for (entity_id, space_id, points) in ballot_points(items, lo, hi) {
            let a = acc.entry(entity_id).or_default();
            a.points += points * weight;
            a.weight += weight;
            a.ballots += 1;
            *a.spaces.entry(space_id).or_insert(0) += 1;
        }
    }

    let mut rows: Vec<(Uuid, Uuid, i64, i32)> = acc
        .into_iter()
        .map(|(entity_id, a)| {
            let score = average_score_key(
                (a.points + AVERAGE_PRIOR_POINTS) / (a.weight + AVERAGE_PRIOR_BALLOTS),
            );
            // Most ballots first, then the lowest space id.
            let space_id = a
                .spaces
                .iter()
                .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
                .map(|(s, _)| *s)
                .expect("an accumulated entity has at least one space");
            (entity_id, space_id, score, a.ballots)
        })
        .collect();
    // Descending score (as an integer key, so equal averages compare equal);
    // then more ballots; then the lower entity id.
    rows.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| b.3.cmp(&a.3))
            .then_with(|| a.0.cmp(&b.0))
    });

    rows.into_iter()
        .take(max_rows)
        .enumerate()
        .map(|(i, (entity_id, space_id, score, ballots))| ScoreRow {
            entity_id,
            space_id,
            // Stored as the rounded value it was ordered by, so two rows with
            // the same stored score are the ones the tie-break ordered.
            score: score as f64 / AVERAGE_SCORE_UNITS,
            position: (i as i32) + 1,
            ballots,
        })
        .collect()
}

/// One ballot's points by position, for [`ScoringMode::Average`]: `(entity,
/// space, points)`, one entry per distinct entity, #1 = `hi` down to last = `lo`
/// (a single entity scores `hi`).
///
/// The rank type and the weights do not set the points. The web publishes
/// WEIGHTED ballots with `0.5 + 0.5 / ln(k + 1)` weights, and min-max scaling
/// those (what [`normalize_ballot`] does) puts the middle of a 300-item list
/// near 0.51 rather than 0.75. The order is the relation positions, which the
/// SDK writes on WEIGHTED and ORDINAL ballots alike; items without a position
/// follow, by weight descending, then entity id. An entity on the ballot twice
/// (under two space ids) keeps its better place and the list is renumbered
/// without the repeat.
pub fn ballot_points(items: &[RankingItem], lo: f64, hi: f64) -> Vec<(Uuid, Uuid, f64)> {
    let mut ordered: Vec<&RankingItem> = items.iter().collect();
    ordered.sort_by(|a, b| {
        (a.position.is_none(), &a.position)
            .cmp(&(b.position.is_none(), &b.position))
            .then_with(|| {
                b.weight
                    .unwrap_or(f64::NEG_INFINITY)
                    .partial_cmp(&a.weight.unwrap_or(f64::NEG_INFINITY))
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.entity_id.cmp(&b.entity_id))
            .then_with(|| a.space_id.cmp(&b.space_id))
    });
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    ordered.retain(|item| seen.insert(item.entity_id));

    let n = ordered.len();
    ordered
        .iter()
        .enumerate()
        .map(|(rank, item)| {
            let points = if n > 1 {
                hi - (hi - lo) * (rank as f64) / ((n - 1) as f64)
            } else {
                hi
            };
            (item.entity_id, item.space_id, points)
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
            block_relation_id: None,
        }
    }

    fn item(entity: u128, position: Option<&str>, weight: Option<f64>) -> RankingItem {
        RankingItem {
            ranking_id: Uuid::from_u128(0),
            entity_id: Uuid::from_u128(entity),
            space_id: Uuid::from_u128(500),
            position: position.map(|s| s.to_string()),
            weight,
            relation_id: None,
            rank_space_id: None,
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
                relation_id: None,
                rank_space_id: None,
            },
            RankingItem {
                ranking_id: Uuid::from_u128(1),
                entity_id: Uuid::from_u128(100),
                space_id: Uuid::from_u128(2),
                position: Some("a1".into()),
                weight: None,
                relation_id: None,
                rank_space_id: None,
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

    #[test]
    fn an_old_ballot_keeps_its_entities_in_the_table() {
        // The GEO-2869 regression, as a test. Two ballots with no entity in
        // common, one far older than the eight half-lives that used to expire
        // it outright. Both sets must appear — the old ones simply sort last.
        let hour = 3600;
        let now = at(400 * 24 * hour);
        let fresh = ranking_submitted(1, "ORDINAL", at(400 * 24 * hour - hour));
        let ancient = ranking_submitted(2, "ORDINAL", at(0)); // ~400 half-lives

        let ballots = vec![
            (
                &fresh,
                vec![item(10, Some("a0"), None), item(11, Some("a1"), None)],
            ),
            (
                &ancient,
                vec![item(20, Some("a0"), None), item(21, Some("a1"), None)],
            ),
        ];
        let rows = aggregate(
            &ballots,
            NORM_LO,
            NORM_HI,
            Some(RecencyDecay {
                half_life_hours: 24.0,
                now,
            }),
        );

        let ids: Vec<Uuid> = rows.iter().map(|r| r.entity_id).collect();
        assert_eq!(rows.len(), 4, "every ranked entity keeps a row");
        for e in [10u128, 11, 20, 21] {
            assert!(ids.contains(&Uuid::from_u128(e)), "entity {e} missing");
        }
        // Ordering still reflects recency: the fresh ballot's entities lead.
        assert_eq!(ids[0], Uuid::from_u128(10));
        assert_eq!(ids[1], Uuid::from_u128(11));
    }

    #[test]
    fn the_projection_is_capped_by_row_count() {
        let r = ranking(1, "WEIGHTED");
        let items: Vec<RankingItem> = (0..(MAX_PROJECTION_ROWS + 25))
            .map(|i| item(1_000 + i as u128, None, Some(1.0 - (i as f64) / 1_000.0)))
            .collect();
        let rows = aggregate(&[(&r, items)], NORM_LO, NORM_HI, None);
        assert_eq!(rows.len(), MAX_PROJECTION_ROWS);
        // Positions stay contiguous from 1 after the cut.
        assert_eq!(rows.first().unwrap().position, 1);
        assert_eq!(rows.last().unwrap().position, MAX_PROJECTION_ROWS as i32);
    }

    // ---- GEO-3676: per-block config, ballot counts, average mode ----

    const AVERAGE: BlockScoring = BlockScoring {
        max_rows: 2_000,
        mode: ScoringMode::Average,
    };

    /// An ordinal ballot over `entities` in order, each under space 500.
    fn ordered(entities: &[u128]) -> Vec<RankingItem> {
        entities
            .iter()
            .enumerate()
            .map(|(i, e)| item(*e, Some(&format!("a{i:04}")), None))
            .collect()
    }

    /// The web's real WEIGHTED ballot: relation positions in order and
    /// `0.5 + 0.5 / ln(k + 1)` weights (rankingVoteWeightFromIndex).
    fn web_weighted(entities: &[u128]) -> Vec<RankingItem> {
        entities
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let k = (i + 1) as f64;
                item(
                    *e,
                    Some(&format!("a{i:04}")),
                    Some(0.5 + 0.5 / (k + 1.0).ln()),
                )
            })
            .collect()
    }

    fn row(rows: &[ScoreRow], entity: u128) -> &ScoreRow {
        rows.iter()
            .find(|r| r.entity_id == Uuid::from_u128(entity))
            .expect("entity has a row")
    }

    #[test]
    fn sum_mode_counts_ballots_per_row() {
        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let rows = aggregate(
            &[(&r1, ordered(&[100, 200])), (&r2, ordered(&[100]))],
            NORM_LO,
            NORM_HI,
            None,
        );
        assert_eq!(row(&rows, 100).ballots, 2);
        assert_eq!(row(&rows, 200).ballots, 1);
    }

    #[test]
    fn the_default_config_is_200_rows_summed() {
        assert_eq!(
            BlockScoring::default(),
            BlockScoring {
                max_rows: MAX_PROJECTION_ROWS,
                mode: ScoringMode::Sum
            }
        );
        assert_eq!(MAX_PROJECTION_ROWS, 200);
    }

    #[test]
    fn a_configured_cap_keeps_more_rows() {
        let r = ranking(1, "ORDINAL");
        let entities: Vec<u128> = (0..2_100).map(|i| 10_000 + i).collect();
        let ballots = [(&r, ordered(&entities))];
        let sum_rows = aggregate_with(
            &ballots,
            NORM_LO,
            NORM_HI,
            None,
            BlockScoring {
                max_rows: 2_000,
                mode: ScoringMode::Sum,
            },
        );
        assert_eq!(sum_rows.len(), 2_000);
        let avg_rows = aggregate_with(&ballots, NORM_LO, NORM_HI, None, AVERAGE);
        assert_eq!(avg_rows.len(), 2_000);
        assert_eq!(avg_rows.last().unwrap().position, 2_000);
        // The default still cuts at 200.
        assert_eq!(aggregate(&ballots, NORM_LO, NORM_HI, None).len(), 200);
    }

    #[test]
    fn average_one_first_place_beats_two_last_places() {
        // Ballot 1 ranks A first of 3; ballots 2 and 3 each rank B last of 3.
        const A: u128 = 100;
        const B: u128 = 200;
        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let r3 = ranking(3, "ORDINAL");
        let ballots = vec![
            (&r1, ordered(&[A, 301, 302])),
            (&r2, ordered(&[303, 304, B])),
            (&r3, ordered(&[305, 306, B])),
        ];

        // Under sums they tie at 1.0.
        let sums = aggregate(&ballots, NORM_LO, NORM_HI, None);
        assert!((row(&sums, A).score - 1.0).abs() < 1e-12);
        assert!((row(&sums, B).score - 1.0).abs() < 1e-12);

        let rows = aggregate_with(&ballots, NORM_LO, NORM_HI, None, AVERAGE);
        // A: (1.0 + 0.75) / 2 = 0.875. B: (0.5 + 0.5 + 0.75) / 3 = 0.5833…,
        // stored rounded to AVERAGE_SCORE_UNITS.
        assert!((row(&rows, A).score - 0.875).abs() < 1e-12);
        assert!((row(&rows, B).score - 1.75 / 3.0).abs() < 1e-9);
        assert_eq!(row(&rows, A).ballots, 1);
        assert_eq!(row(&rows, B).ballots, 2);
        assert!(row(&rows, A).position < row(&rows, B).position);
        assert_eq!(rows[0].entity_id, Uuid::from_u128(A));
    }

    #[test]
    fn average_scores_the_webs_weighted_ballot_like_the_same_order_as_ordinal() {
        let order: Vec<u128> = (0..300).map(|i| 1_000 + i).collect();
        let weighted = ranking(1, "WEIGHTED");
        let ordinal = ranking(2, "ORDINAL");

        let from_weighted = aggregate_with(
            &[(&weighted, web_weighted(&order))],
            NORM_LO,
            NORM_HI,
            None,
            AVERAGE,
        );
        let from_ordinal = aggregate_with(
            &[(&ordinal, ordered(&order))],
            NORM_LO,
            NORM_HI,
            None,
            AVERAGE,
        );
        assert_eq!(from_weighted, from_ordinal);

        // The middle of the list scores mid-list (0.75 before the prior), not
        // the ~0.51 min-max scaling of those weights gives.
        let middle = order[149];
        let points = ballot_points(&web_weighted(&order), NORM_LO, NORM_HI);
        let p = points
            .iter()
            .find(|(e, _, _)| *e == Uuid::from_u128(middle))
            .unwrap()
            .2;
        assert!((p - 0.75).abs() < 0.002, "middle place scored {p}");
        let min_max = normalize_ballot(&weighted, &web_weighted(&order), NORM_LO, NORM_HI);
        assert!(value_for(&min_max, middle) < 0.52);
    }

    #[test]
    fn average_orders_a_ballot_without_positions_by_weight() {
        let r = ranking(1, "WEIGHTED");
        let items = vec![
            item(12, None, Some(0.6)),
            item(10, None, Some(0.9)),
            item(11, None, Some(0.7)),
        ];
        let points = ballot_points(&items, NORM_LO, NORM_HI);
        let order: Vec<Uuid> = points.iter().map(|(e, _, _)| *e).collect();
        assert_eq!(
            order,
            vec![
                Uuid::from_u128(10),
                Uuid::from_u128(11),
                Uuid::from_u128(12)
            ]
        );
        let rows = aggregate_with(&[(&r, items)], NORM_LO, NORM_HI, None, AVERAGE);
        assert!((row(&rows, 10).score - (1.0 + 0.75) / 2.0).abs() < 1e-12);
        assert!((row(&rows, 12).score - (0.5 + 0.75) / 2.0).abs() < 1e-12);
    }

    #[test]
    fn average_keys_on_entity_only() {
        // One debate referenced under two space ids, on separate ballots and
        // twice on one ballot, is one row.
        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let r3 = ranking(3, "ORDINAL");
        let under = |space: u128, pos: &str| RankingItem {
            ranking_id: Uuid::from_u128(0),
            entity_id: Uuid::from_u128(100),
            space_id: Uuid::from_u128(space),
            position: Some(pos.into()),
            weight: None,
            relation_id: None,
            rank_space_id: None,
        };
        let ballots = vec![
            (&r1, vec![under(7, "a0"), item(200, Some("a1"), None)]),
            (&r2, vec![under(7, "a0"), item(200, Some("a1"), None)]),
            // Twice on one ballot: counts once, at its better place, and the
            // list is renumbered without the repeat (A first of 2).
            (
                &r3,
                vec![under(3, "a0"), item(200, Some("a1"), None), under(7, "a2")],
            ),
        ];
        let rows = aggregate_with(&ballots, NORM_LO, NORM_HI, None, AVERAGE);
        assert_eq!(rows.len(), 2);
        let a = row(&rows, 100);
        assert_eq!(a.ballots, 3);
        assert!((a.score - (3.0 + 0.75) / 4.0).abs() < 1e-12);
        // Space 7 carried it on two ballots, space 3 on one.
        assert_eq!(a.space_id, Uuid::from_u128(7));

        // Sum mode is unchanged: two perspectives stay two rows.
        let sums = aggregate(&ballots, NORM_LO, NORM_HI, None);
        assert_eq!(
            sums.iter()
                .filter(|r| r.entity_id == Uuid::from_u128(100))
                .count(),
            2
        );
    }

    #[test]
    fn average_row_space_tie_goes_to_the_lowest_space_id() {
        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let under = |space: u128| RankingItem {
            ranking_id: Uuid::from_u128(0),
            entity_id: Uuid::from_u128(100),
            space_id: Uuid::from_u128(space),
            position: Some("a0".into()),
            weight: None,
            relation_id: None,
            rank_space_id: None,
        };
        let rows = aggregate_with(
            &[(&r1, vec![under(9)]), (&r2, vec![under(4)])],
            NORM_LO,
            NORM_HI,
            None,
            AVERAGE,
        );
        assert_eq!(rows[0].space_id, Uuid::from_u128(4));
    }

    #[test]
    fn average_ties_go_to_more_ballots_then_lower_entity_id() {
        // Three exact ties at 0.75:
        //   D (400): first of 2, then last of 2 -> (1.0 + 0.5 + 0.75) / 3
        //   E (500): middle of 3 on one ballot  -> (0.75 + 0.75) / 2
        //   F (50):  middle of 3 on one ballot  -> (0.75 + 0.75) / 2
        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let r3 = ranking(3, "ORDINAL");
        let r4 = ranking(4, "ORDINAL");
        let ballots = vec![
            (&r1, ordered(&[400, 900])),      // D first of 2
            (&r2, ordered(&[901, 400])),      // D last of 2
            (&r3, ordered(&[902, 500, 903])), // E middle of 3
            (&r4, ordered(&[904, 50, 905])),  // F (50) middle of 3
        ];
        let rows = aggregate_with(&ballots, NORM_LO, NORM_HI, None, AVERAGE);
        let pos = |e: u128| row(&rows, e).position;
        for e in [400, 500, 50] {
            assert!((row(&rows, e).score - 0.75).abs() < 1e-12, "entity {e}");
        }
        // D is on two ballots: above E and F, though F has the lowest id.
        assert!(pos(400) < pos(50));
        assert!(pos(400) < pos(500));
        // E and F tie on score and ballots: the lower entity id leads.
        assert!(pos(50) < pos(500));
    }

    #[test]
    fn average_ties_that_differ_by_rounding_still_go_to_more_ballots() {
        // X is 5th of 6 on one ballot: (0.6 + 0.75) / 2 = 0.675.
        // Y is 4th of 5 and 8th of 11: (0.625 + 0.65 + 0.75) / 3 = 0.675,
        // which raw f64 arithmetic makes 0.6749999999999999.
        // They are equal, so Y (two ballots) leads, though X has the lower id.
        const X: u128 = 100;
        const Y: u128 = 200;
        let filler =
            |from: u128, n: usize| -> Vec<u128> { (0..n as u128).map(|i| from + i).collect() };
        let mut x_ballot = filler(1_000, 4);
        x_ballot.extend([X, 1_100]);
        let mut y_short = filler(2_000, 3);
        y_short.extend([Y, 2_100]);
        let mut y_long = filler(3_000, 7);
        y_long.push(Y);
        y_long.extend(filler(3_100, 3));
        assert_eq!((x_ballot.len(), y_short.len(), y_long.len()), (6, 5, 11));

        let r1 = ranking(1, "ORDINAL");
        let r2 = ranking(2, "ORDINAL");
        let r3 = ranking(3, "ORDINAL");
        let ballots = vec![
            (&r1, ordered(&x_ballot)),
            (&r2, ordered(&y_short)),
            (&r3, ordered(&y_long)),
        ];
        let rows = aggregate_with(&ballots, NORM_LO, NORM_HI, None, AVERAGE);
        let (x, y) = (row(&rows, X), row(&rows, Y));
        assert_eq!((x.ballots, y.ballots), (1, 2));
        // The stored scores are the same number, so they agree with the order.
        assert_eq!(x.score, y.score);
        assert!((x.score - 0.675).abs() < 1e-12);
        assert!(y.position < x.position, "more ballots wins an equal score");
    }

    #[test]
    fn average_decays_points_and_count_together_on_a_rolling_block() {
        let hour = 3600;
        let now = at(48 * hour);
        let fresh = ranking_submitted(1, "ORDINAL", now);
        let stale = ranking_submitted(2, "ORDINAL", at(0)); // two half-lives: 0.25
        let ballots = vec![
            (&fresh, ordered(&[100, 200])),
            (&stale, ordered(&[200, 100])),
        ];
        let rows = aggregate_with(
            &ballots,
            NORM_LO,
            NORM_HI,
            Some(RecencyDecay {
                half_life_hours: 24.0,
                now,
            }),
            AVERAGE,
        );
        // A: (1.0 + 0.25*0.5 + 0.75) / (1 + 0.25 + 1)
        let a = (1.0 + 0.125 + 0.75) / 2.25;
        assert!((row(&rows, 100).score - a).abs() < 1e-9);
        // The count is of ballots, undecayed.
        assert_eq!(row(&rows, 100).ballots, 2);
        assert_eq!(rows[0].entity_id, Uuid::from_u128(100));
    }

    #[test]
    fn scoring_mode_round_trips_its_stored_spelling() {
        for mode in [ScoringMode::Sum, ScoringMode::Average] {
            assert_eq!(ScoringMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(ScoringMode::parse("mean"), None);
    }
}
