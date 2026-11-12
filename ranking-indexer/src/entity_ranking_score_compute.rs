//! The scoring arithmetic of `public.refresh_entity_ranking_scores` (migration 0113), as Rust functions.
//!
//! The SQL function does two things: it reads a pile of tables (votes_count, relations, values, the type
//! weights, the topic, watch and trending term functions) into one row of counts per entity, and it turns
//! those counts into the stored scores. This module is the second half, the part with no table in it: the
//! four helper functions, the composition `ranking_score = (entity_ranking_score [+ topic_score]) +
//! watch_score + age_credit + trending_score`, and the pure arithmetic of `trending.entity_trending_terms`
//! (the age credit, the listing and topic boosts and which of them is the source). The reads, the upsert and
//! the two reconciles of `entity_type_ranking` and `entity_topic_ranking` stay in SQL.
//!
//! | Rust | SQL | migration |
//! | -- | -- | -- |
//! | [`intrinsic_score`] | `public.entity_intrinsic_score` | 0073 |
//! | [`participation_score`] | `public.entity_participation_score` | 0078 |
//! | [`ranking_score`] | `public.entity_ranking_score` | 0079 |
//! | [`score`] | the `computed` CTE and the upsert's `CASE` | 0113 |
//! | [`trending_terms`] | the `inputs` and `terms` CTEs of `trending.entity_trending_terms` | 0113, 0118, 0122, 0123 |
//!
//! `public.wilson_lower_bound` is [`crate::sql_helpers::wilson_lower_bound`].
//!
//! Pinned to the SQL by `tests/fixtures/entity_ranking_score_golden.txt`: 2,736 rows that a scratch Postgres
//! 16 returned from the migrations' own function definitions (the `G` rows, 1,536 of them, are the listing score
//! with the position term of migration 0122: the same `CASE`, evaluated in `numeric` over configs, days, places
//! and day counts). The SQL does this in `numeric` and the twin in
//! `f64`, so the comparison is to 1e-9 (relative above 1), not bitwise; the largest terms are about 4e4.
//!
//! A shadow in `agenda_listings_refresh` (flag `ENTITY_RANKING_SCORE_RUST_COMPUTE`, default off) recomputes
//! recently written rows from the inputs stored beside them and counts the rows that disagree.

use crate::sql_helpers::wilson_lower_bound;

/// The columns of `entity_ranking_config` the scoring reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub tau_seconds: f64,
    pub wilson_z: f64,
    pub prior_positive: f64,
    pub prior_negative: f64,
    pub quality_floor: f64,
    pub intrinsic_cap: f64,
    pub intrinsic_property_target: i32,
    pub intrinsic_relation_target: i32,
    pub participation_weight: f64,
    pub participation_cap: f64,
    pub comment_weight: f64,
    pub comment_cap: f64,
}

/// One entity's inputs: what the `computed` CTE's joins produce before any scoring.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Inputs {
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub property_count: i32,
    pub relation_count: i32,
    /// `None` when no type of the entity has a weight (the SQL then uses 1.0).
    pub type_weight: Option<f64>,
    /// `Some` only for a topic debate.
    pub topic_score: Option<f64>,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub created_epoch: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored {
    pub quality: f64,
    pub intrinsic: f64,
    pub participation: f64,
    pub comment: f64,
    pub ranking: f64,
}

/// `public.entity_intrinsic_score`: a small bonus from how many properties and relations the entity has,
/// capped at `cap`. Each count saturates at its target.
pub fn intrinsic_score(
    property_count: Option<i32>,
    relation_count: Option<i32>,
    cap: Option<f64>,
    property_target: Option<i32>,
    relation_target: Option<i32>,
) -> f64 {
    let cap = cap.unwrap_or(0.5);
    let share = |count: Option<i32>, target: Option<i32>, default: i32| {
        let c = f64::from(count.unwrap_or(0).max(0));
        let t = f64::from(target.unwrap_or(default).max(1));
        (c / t).min(1.0)
    };
    cap.min(
        cap * (0.6 * share(property_count, property_target, 10)
            + 0.4 * share(relation_count, relation_target, 5)),
    )
}

/// `public.entity_participation_score`: `weight * ln(1 + n)`, capped, with `n = 0` worth 0.
pub fn participation_score(
    positions_total: Option<i64>,
    weight: Option<f64>,
    cap: Option<f64>,
) -> f64 {
    let n = positions_total.unwrap_or(0).max(0) as f64;
    cap.unwrap_or(10.0)
        .max(0.0)
        .min(weight.unwrap_or(0.0).max(0.0) * (1.0 + n).ln())
}

/// `public.entity_ranking_score` (the 8-argument form from 0079): log quality plus log type weight plus the
/// three additive terms plus the creation time over tau. Missing arguments take the SQL's defaults.
#[allow(clippy::too_many_arguments)]
pub fn ranking_score(
    quality_score: Option<f64>,
    type_weight: Option<f64>,
    intrinsic_score: Option<f64>,
    participation_score: Option<f64>,
    comment_score: Option<f64>,
    created_at_epoch: Option<i64>,
    tau_seconds: Option<f64>,
    quality_floor: Option<f64>,
) -> f64 {
    let floor = quality_floor.unwrap_or(0.01).max(1e-9);
    quality_score.unwrap_or(0.0).max(floor).ln()
        + type_weight.unwrap_or(1.0).max(1e-9).ln()
        + intrinsic_score.unwrap_or(0.0)
        + participation_score.unwrap_or(0.0)
        + comment_score.unwrap_or(0.0)
        + created_at_epoch.unwrap_or(0) as f64 / tau_seconds.unwrap_or(45000.0).max(1.0)
}

/// The scores `refresh_entity_ranking_scores` writes for one entity.
pub fn score(cfg: &Config, i: &Inputs) -> Scored {
    let quality = wilson_lower_bound(
        Some(i.positive as f64),
        Some(i.negative as f64),
        Some(cfg.wilson_z),
        Some(cfg.prior_positive),
        Some(cfg.prior_negative),
    );
    let intrinsic = intrinsic_score(
        Some(i.property_count),
        Some(i.relation_count),
        Some(cfg.intrinsic_cap),
        Some(cfg.intrinsic_property_target),
        Some(cfg.intrinsic_relation_target),
    );
    let participation = participation_score(
        Some(i.stance_positive + i.stance_negative + i.positive + i.negative),
        Some(cfg.participation_weight),
        Some(cfg.participation_cap),
    );
    let comment = participation_score(
        Some(i.commenter_count),
        Some(cfg.comment_weight),
        Some(cfg.comment_cap),
    );
    let base = ranking_score(
        Some(quality),
        Some(i.type_weight.unwrap_or(1.0)),
        Some(intrinsic),
        Some(participation),
        Some(comment),
        i.created_epoch,
        Some(cfg.tau_seconds),
        Some(cfg.quality_floor),
    );
    let ranking =
        base + i.topic_score.unwrap_or(0.0) + i.watch_score + i.age_credit + i.trending_score;
    Scored {
        quality,
        intrinsic,
        participation,
        comment,
        ranking,
    }
}

/// The `trending.entity_trending_terms` arithmetic. Day counts and epochs come from the caller (the
/// `sql_helpers` date functions give them): `days` is today minus the last listed date, `topic_days` today
/// minus the topic listing's date.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrendConfig {
    pub boost: f64,
    pub topic_boost: f64,
    pub day_factor: f64,
    pub debate_factor: f64,
    pub horizon_days: i32,
    pub tau_seconds: f64,
    /// `trending_position_base`, `_step`, `_slots` and `_day_factor` (migration 0122).
    pub position_base: f64,
    pub position_step: f64,
    pub position_slots: i32,
    pub position_day_factor: f64,
    /// `ranking_places_mode = 2` (migration 0123): the ranking place and the push-down are served and the agenda
    /// position term is not. Off and shadow are both `false`: they score exactly as 0122.
    pub ranking_on: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrendInputs {
    /// There is an agenda listing for the entity.
    pub listed: bool,
    pub days: Option<i64>,
    /// Debates in the window on a listed claim; `None` for a debate.
    pub debates: Option<i64>,
    /// The entity is tagged with a topic an agenda names, and this many days since that listing.
    pub topic_days: Option<i64>,
    /// Midnight of the last listed date in the trending time zone, as an epoch.
    pub listed_epoch: Option<i64>,
    pub created_epoch: Option<i64>,
    /// A debate's place on its newest ordered day (`trending.debate_positions`), and the days since that day.
    pub position: Option<i32>,
    pub position_days: Option<i64>,
    /// The debate's best weighted Best debates place value (`trending.ranking_place_values`, 0123), 0 when unplaced.
    /// Read only while `ranking_on`.
    pub ranking_place: f64,
    /// `ranking_pushdown` for a pushed-down debate (`trending.ranking_pushdowns`, 0123), else 0. Read only while
    /// `ranking_on`.
    pub pushdown: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrendTerms {
    pub age_credit: f64,
    pub trending_score: f64,
    /// `"listing"`, `"topic"` or none.
    pub source: Option<&'static str>,
    pub listing_score: f64,
    pub topic_score: f64,
}

/// `(base + step × (slots − place + 1)) × day_factor^days` for a place within the slots whose day is inside
/// the horizon, else 0. Added to the listing score only while the debate's own listing is inside the horizon.
fn position_term(cfg: &TrendConfig, t: &TrendInputs, beyond: &impl Fn(i64) -> bool) -> f64 {
    match (t.position, t.position_days) {
        (Some(place), Some(days)) if place <= cfg.position_slots && !beyond(days) => {
            (cfg.position_base + cfg.position_step * f64::from(cfg.position_slots - place + 1))
                * cfg.position_day_factor.powi(days as i32)
        }
        _ => 0.0,
    }
}

pub fn trending_terms(cfg: &TrendConfig, t: &TrendInputs) -> TrendTerms {
    let age_credit = if t.listed {
        let listed = t.listed_epoch.unwrap_or(0) as f64;
        (((listed - t.created_epoch.unwrap_or(0) as f64) / cfg.tau_seconds.max(1.0)).max(0.0))
            .max(0.0)
    } else {
        0.0
    };
    let beyond = |days: i64| days > i64::from(cfg.horizon_days);
    let listing_score = match (t.listed, t.days) {
        (true, Some(d)) if !beyond(d) => {
            cfg.boost
                * cfg.day_factor.powi(d as i32)
                * cfg.debate_factor.powi(t.debates.unwrap_or(0) as i32)
                + if cfg.ranking_on {
                    0.0
                } else {
                    position_term(cfg, t, &beyond)
                }
        }
        _ => 0.0,
    };
    let topic_score = match t.topic_days {
        Some(d) if !beyond(d) => cfg.topic_boost * cfg.day_factor.powi(d as i32),
        _ => 0.0,
    };
    // 0123: the ranking place inside the GREATEST (reported as a listing), the push-down after it.
    let (place, pushdown) = if cfg.ranking_on {
        (t.ranking_place, t.pushdown)
    } else {
        (0.0, 0.0)
    };
    let listed = listing_score.max(place);
    let source = if listed > 0.0 && listed >= topic_score {
        Some("listing")
    } else if topic_score > 0.0 {
        Some("topic")
    } else {
        None
    };
    TrendTerms {
        age_credit,
        trending_score: listing_score.max(topic_score).max(place) - pushdown,
        source,
        listing_score,
        topic_score,
    }
}

/// A row of `entity_ranking_scores` with the created time of its entity: the shadow's input and what it
/// compares to.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Stored {
    pub entity_id: uuid::Uuid,
    pub quality_score: f64,
    pub intrinsic_score: f64,
    pub participation_score: f64,
    pub comment_score: f64,
    pub ranking_score: f64,
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub type_weight: f64,
    pub topic_score: f64,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub created_epoch: Option<i64>,
}

/// The config read, with every numeric cast to float8.
pub const CONFIG_SQL: &str = "SELECT tau_seconds::float8, wilson_z::float8, prior_positive::float8, \
     prior_negative::float8, quality_floor::float8, intrinsic_cap::float8, intrinsic_property_target, \
     intrinsic_relation_target, participation_weight::float8, participation_cap::float8, \
     comment_weight::float8, comment_cap::float8 FROM public.entity_ranking_config WHERE id";

/// The most recently written rows, newest first.
pub const STORED_SQL: &str = "SELECT s.entity_id, s.quality_score::float8 AS quality_score, \
     s.intrinsic_score::float8 AS intrinsic_score, s.participation_score::float8 AS participation_score, \
     s.comment_score::float8 AS comment_score, s.ranking_score::float8 AS ranking_score, s.positive, \
     s.negative, s.stance_positive::bigint AS stance_positive, s.stance_negative::bigint AS stance_negative, \
     s.commenter_count, s.type_weight::float8 AS type_weight, s.topic_score::float8 AS topic_score, \
     s.watch_score::float8 AS watch_score, s.age_credit::float8 AS age_credit, \
     s.trending_score::float8 AS trending_score, \
     CASE WHEN e.created_at ~ '^[0-9]+$' THEN e.created_at::bigint END AS created_epoch \
     FROM public.entity_ranking_scores s JOIN public.entities e ON e.id = s.entity_id \
     ORDER BY s.updated_at DESC LIMIT $1";

pub fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * b.abs().max(1.0)
}

/// Rows whose stored scores differ from the twin's, given the inputs stored beside them. The property and
/// relation counts are not stored, so the stored intrinsic score is taken as given.
pub fn mismatches(cfg: &Config, rows: &[Stored]) -> Vec<uuid::Uuid> {
    rows.iter()
        .filter(|r| {
            let base = ranking_score(
                Some(wilson_lower_bound(
                    Some(r.positive as f64),
                    Some(r.negative as f64),
                    Some(cfg.wilson_z),
                    Some(cfg.prior_positive),
                    Some(cfg.prior_negative),
                )),
                Some(r.type_weight),
                Some(r.intrinsic_score),
                Some(participation_score(
                    Some(r.stance_positive + r.stance_negative + r.positive + r.negative),
                    Some(cfg.participation_weight),
                    Some(cfg.participation_cap),
                )),
                Some(participation_score(
                    Some(r.commenter_count),
                    Some(cfg.comment_weight),
                    Some(cfg.comment_cap),
                )),
                r.created_epoch,
                Some(cfg.tau_seconds),
                Some(cfg.quality_floor),
            );
            let ranking = base + r.topic_score + r.watch_score + r.age_credit + r.trending_score;
            let quality = wilson_lower_bound(
                Some(r.positive as f64),
                Some(r.negative as f64),
                Some(cfg.wilson_z),
                Some(cfg.prior_positive),
                Some(cfg.prior_negative),
            );
            let part = participation_score(
                Some(r.stance_positive + r.stance_negative + r.positive + r.negative),
                Some(cfg.participation_weight),
                Some(cfg.participation_cap),
            );
            let comment = participation_score(
                Some(r.commenter_count),
                Some(cfg.comment_weight),
                Some(cfg.comment_cap),
            );
            !(close(ranking, r.ranking_score)
                && close(quality, r.quality_score)
                && close(part, r.participation_score)
                && close(comment, r.comment_score))
        })
        .map(|r| r.entity_id)
        .collect()
}

/// `ENTITY_RANKING_SCORE_RUST_COMPUTE`: on for `1` or `true`. Default off.
pub fn shadow_enabled() -> bool {
    matches!(
        std::env::var("ENTITY_RANKING_SCORE_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/entity_ranking_score_golden.txt");

    /// `(inputs, outputs)` of each row of one kind; the inputs split on `,`, the outputs on `|`.
    fn rows(kind: &str) -> Vec<(Vec<&'static str>, Vec<&'static str>)> {
        GOLDEN
            .lines()
            .filter(|l| l.split('|').next() == Some(kind))
            .map(|l| {
                let mut p = l.split('|').skip(1);
                let ins = p.next().unwrap().split(',').collect();
                (ins, p.collect())
            })
            .collect()
    }

    fn f(s: &str) -> Option<f64> {
        (!s.is_empty()).then(|| s.parse().expect("a number"))
    }
    fn i(s: &str) -> Option<i64> {
        (!s.is_empty()).then(|| s.parse().expect("an integer"))
    }
    fn want(s: &str) -> f64 {
        s.parse().expect("a number")
    }
    fn check(what: &str, got: f64, sql: &str, ctx: &dyn std::fmt::Debug) {
        assert!(close(got, want(sql)), "{what}{ctx:?}: got {got}, SQL {sql}");
    }

    #[test]
    fn intrinsic_matches_the_sql() {
        let g = rows("I");
        assert!(g.len() >= 150, "grid missing: {}", g.len());
        for (a, o) in g {
            let got = intrinsic_score(
                i(a[0]).map(|v| v as i32),
                i(a[1]).map(|v| v as i32),
                f(a[2]),
                i(a[3]).map(|v| v as i32),
                i(a[4]).map(|v| v as i32),
            );
            check("intrinsic", got, o[0], &a);
        }
    }

    #[test]
    fn participation_matches_the_sql() {
        let g = rows("P");
        assert!(g.len() >= 150, "grid missing: {}", g.len());
        for (a, o) in g {
            check(
                "participation",
                participation_score(i(a[0]), f(a[1]), f(a[2])),
                o[0],
                &a,
            );
        }
    }

    #[test]
    fn ranking_score_matches_the_sql() {
        let g = rows("R");
        assert!(g.len() >= 200, "grid missing: {}", g.len());
        for (a, o) in g {
            let got = ranking_score(
                f(a[0]),
                f(a[1]),
                f(a[2]),
                f(a[3]),
                f(a[4]),
                i(a[5]),
                f(a[6]),
                f(a[7]),
            );
            check("ranking_score", got, o[0], &a);
        }
    }

    #[test]
    fn composition_matches_the_sql() {
        let g = rows("C");
        assert!(g.len() >= 400, "grid missing: {}", g.len());
        for (a, o) in g {
            let n = |k: usize| f(a[k]).unwrap();
            let cfg = Config {
                tau_seconds: n(0),
                wilson_z: n(1),
                prior_positive: n(2),
                prior_negative: n(3),
                quality_floor: n(4),
                intrinsic_cap: n(5),
                intrinsic_property_target: n(6) as i32,
                intrinsic_relation_target: n(7) as i32,
                participation_weight: n(8),
                participation_cap: n(9),
                comment_weight: n(10),
                comment_cap: n(11),
            };
            let inp = Inputs {
                positive: i(a[12]).unwrap(),
                negative: i(a[13]).unwrap(),
                stance_positive: i(a[14]).unwrap(),
                stance_negative: i(a[15]).unwrap(),
                commenter_count: i(a[16]).unwrap(),
                property_count: i(a[17]).unwrap() as i32,
                relation_count: i(a[18]).unwrap() as i32,
                type_weight: f(a[19]),
                topic_score: f(a[20]),
                watch_score: n(21),
                age_credit: n(22),
                trending_score: n(23),
                created_epoch: i(a[24]),
            };
            let s = score(&cfg, &inp);
            check("quality", s.quality, o[0], &a);
            check("intrinsic", s.intrinsic, o[1], &a);
            check("participation", s.participation, o[2], &a);
            check("comment", s.comment, o[3], &a);
            check("ranking", s.ranking, o[4], &a);
        }
    }

    /// The position fields of a config that has no position term: the T grid has no positions anyway.
    const POSITIONS_OFF: TrendConfig = TrendConfig {
        boost: 0.0,
        topic_boost: 0.0,
        day_factor: 0.0,
        debate_factor: 0.0,
        horizon_days: 0,
        tau_seconds: 1.0,
        position_base: 2500.0,
        position_step: 100.0,
        position_slots: 50,
        position_day_factor: 0.3,
        ranking_on: false,
    };

    /// The listing score with the position term (migration 0122), over a grid of configs, days, places and
    /// day counts, as `numeric` evaluated the CASE in a scratch Postgres 16.
    #[test]
    fn position_term_matches_the_sql() {
        let g = rows("G");
        assert!(g.len() >= 1500, "grid missing: {}", g.len());
        let (mut placed, mut lifted) = (0, 0);
        for (a, o) in g {
            let cfg = TrendConfig {
                boost: f(a[0]).unwrap(),
                day_factor: f(a[1]).unwrap(),
                debate_factor: f(a[2]).unwrap(),
                horizon_days: i(a[5]).unwrap() as i32,
                position_base: f(a[6]).unwrap(),
                position_step: f(a[7]).unwrap(),
                position_slots: i(a[8]).unwrap() as i32,
                position_day_factor: f(a[9]).unwrap(),
                ..POSITIONS_OFF
            };
            let t = TrendInputs {
                listed: true,
                days: i(a[4]),
                debates: i(a[3]),
                listed_epoch: Some(0),
                created_epoch: Some(0),
                position: a.get(10).and_then(|x| i(x)).map(|x| x as i32),
                position_days: a.get(11).and_then(|x| i(x)),
                ..Default::default()
            };
            let r = trending_terms(&cfg, &t);
            check("listing", r.listing_score, o[0], &a);
            check("trending", r.trending_score, o[0], &a);
            placed += i32::from(t.position.is_some());
            lifted += i32::from(r.listing_score > 100.0);
        }
        assert!(placed > 1000 && lifted > 200, "{placed} {lifted}");
    }

    #[test]
    fn trending_terms_match_the_sql() {
        let g = rows("T");
        assert!(g.len() >= 300, "grid missing: {}", g.len());
        for (a, o) in g {
            let cfg = TrendConfig {
                boost: f(a[0]).unwrap(),
                topic_boost: f(a[1]).unwrap(),
                day_factor: f(a[2]).unwrap(),
                debate_factor: f(a[3]).unwrap(),
                horizon_days: i(a[4]).unwrap() as i32,
                tau_seconds: f(a[5]).unwrap(),
                ..POSITIONS_OFF
            };
            let t = TrendInputs {
                listed: i(a[9]).is_some(),
                days: i(a[6]),
                debates: i(a[7]),
                topic_days: i(a[8]),
                listed_epoch: i(a[9]),
                created_epoch: i(a[10]),
                ..Default::default()
            };
            let r = trending_terms(&cfg, &t);
            check("age_credit", r.age_credit, o[0], &a);
            check("trending", r.trending_score, o[1], &a);
            assert_eq!(r.source.unwrap_or(""), o[2], "source{a:?}");
            check("listing", r.listing_score, o[3], &a);
            check("topic", r.topic_score, o[4], &a);
        }
    }

    #[test]
    fn mismatches_flags_a_changed_row_only() {
        let cfg = Config {
            tau_seconds: 45000.0,
            wilson_z: 1.96,
            prior_positive: 1.0,
            prior_negative: 1.0,
            quality_floor: 0.01,
            intrinsic_cap: 0.5,
            intrinsic_property_target: 10,
            intrinsic_relation_target: 5,
            participation_weight: 1.5,
            participation_cap: 10.0,
            comment_weight: 2.0,
            comment_cap: 30.0,
        };
        let inp = Inputs {
            positive: 5,
            negative: 1,
            commenter_count: 3,
            type_weight: Some(1.5),
            created_epoch: Some(1_762_400_000),
            ..Default::default()
        };
        let s = score(&cfg, &inp);
        let row = Stored {
            entity_id: uuid::Uuid::nil(),
            quality_score: s.quality,
            intrinsic_score: s.intrinsic,
            participation_score: s.participation,
            comment_score: s.comment,
            ranking_score: s.ranking,
            positive: 5,
            negative: 1,
            stance_positive: 0,
            stance_negative: 0,
            commenter_count: 3,
            type_weight: 1.5,
            topic_score: 0.0,
            watch_score: 0.0,
            age_credit: 0.0,
            trending_score: 0.0,
            created_epoch: Some(1_762_400_000),
        };
        // intrinsic is stored, not recomputed, so use the one `score` gave for zero counts
        assert!(mismatches(&cfg, std::slice::from_ref(&row)).is_empty());
        let mut bad = row.clone();
        bad.ranking_score += 0.5;
        assert_eq!(mismatches(&cfg, &[row, bad]).len(), 1);
    }
}
