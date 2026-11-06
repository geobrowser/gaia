//! The score row of `public.entity_ranking_scores` as a pure function, and the engaged-only
//! overlay in geo-db collection 21 that sits on top of it (plan 10.21 section 9).
//!
//! An entity nobody engaged with has a row that is a function of its card (distinct property
//! count, non-system outgoing relation count, `created_at`, the heaviest weight among its Types)
//! and three config records (`entity_ranking_config`, `entity_type_weights`): twelve of the
//! fourteen columns are 0, NULL or one config constant, and `intrinsic_score` and `ranking_score`
//! are [`default_row`]. Only an entity with a nonzero engagement term is stored, in collection 21,
//! in the layout of [`Overlay`]; an absent key means "build the default from the card".
//!
//! The functions here are pinned to the SQL (`wilson_lower_bound`, `entity_intrinsic_score`,
//! `entity_participation_score`, `entity_ranking_score` of migrations 0073, 0078, 0079; assembled
//! as in `refresh_entity_ranking_scores`, 0113) by `entity_scores_fixture`.

use uuid::Uuid;

/// `entity_ranking_config`, the columns the score reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub tau_seconds: f64,
    pub wilson_z: f64,
    pub prior_positive: f64,
    pub prior_negative: f64,
    pub quality_floor: f64,
    pub intrinsic_cap: f64,
    pub intrinsic_property_target: i64,
    pub intrinsic_relation_target: i64,
    pub participation_weight: f64,
    pub participation_cap: f64,
    pub comment_weight: f64,
    pub comment_cap: f64,
}

/// What the card contributes. `type_weights` are the `entity_type_weights.weight` of each Types
/// relation that has a row (empty when none does); `created_epoch` is `entities.created_at` as
/// seconds, `None` when it is empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub property_count: i64,
    pub relation_count: i64,
    pub created_epoch: Option<i64>,
    pub type_weights: Vec<f64>,
}

/// `COALESCE(MAX(weight), 1.0)`.
pub fn type_weight(card: &Card) -> f64 {
    card.type_weights
        .iter()
        .copied()
        .fold(None, |m: Option<f64>, w| Some(m.map_or(w, |m| m.max(w))))
        .unwrap_or(1.0)
}

/// `wilson_lower_bound(positive, negative, z, prior_positive, prior_negative)`.
pub fn wilson_lower_bound(positive: i64, negative: i64, c: &Config) -> f64 {
    let p_obs = positive.max(0) as f64 + c.prior_positive.max(0.0);
    let n_obs = negative.max(0) as f64 + c.prior_negative.max(0.0);
    let total = p_obs + n_obs;
    if total <= 0.0 {
        return 0.0;
    }
    let z = c.wilson_z;
    let phat = p_obs / total;
    let lower = (phat + z * z / (2.0 * total)
        - z * ((phat * (1.0 - phat) + z * z / (4.0 * total)) / total).sqrt())
        / (1.0 + z * z / total);
    lower.clamp(0.0, 1.0)
}

/// `entity_intrinsic_score(props, rels, cap, property_target, relation_target)`.
pub fn intrinsic_score(props: i64, rels: i64, c: &Config) -> f64 {
    let pt = c.intrinsic_property_target.max(1) as f64;
    let rt = c.intrinsic_relation_target.max(1) as f64;
    let p = (props.max(0) as f64 / pt).min(1.0);
    let r = (rels.max(0) as f64 / rt).min(1.0);
    c.intrinsic_cap.min(c.intrinsic_cap * (0.6 * p + 0.4 * r))
}

/// `entity_participation_score(total, weight, cap)`; used for both the participation and the
/// comment term with their own weight and cap.
pub fn participation_score(total: i64, weight: f64, cap: f64) -> f64 {
    cap.max(0.0)
        .min(weight.max(0.0) * (1.0 + total.max(0) as f64).ln())
}

/// The un-engaged-or-engaged base: `entity_ranking_score(...)` without the topic, watch, age
/// credit and trending terms.
pub fn base_score(
    quality: f64,
    tw: f64,
    intrinsic: f64,
    participation: f64,
    comment: f64,
    created: Option<i64>,
    c: &Config,
) -> f64 {
    quality.max(c.quality_floor.max(1e-9)).ln()
        + tw.max(1e-9).ln()
        + intrinsic
        + participation
        + comment
        + created.unwrap_or(0) as f64 / c.tau_seconds.max(1.0)
}

/// The columns a reader needs for an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Columns {
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub quality_score: f64,
    pub intrinsic_score: f64,
    pub participation_score: f64,
    pub comment_score: f64,
    pub type_weight: f64,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub trending_source: Option<TrendingSource>,
    pub trending_topic_id: Option<Uuid>,
    pub ranking_score: f64,
}

/// The default row for an entity with no overlay row.
pub fn default_row(card: &Card, c: &Config) -> Columns {
    let quality = wilson_lower_bound(0, 0, c);
    let intrinsic = intrinsic_score(card.property_count, card.relation_count, c);
    let tw = type_weight(card);
    Columns {
        positive: 0,
        negative: 0,
        stance_positive: 0,
        stance_negative: 0,
        commenter_count: 0,
        quality_score: quality,
        intrinsic_score: intrinsic,
        participation_score: 0.0,
        comment_score: 0.0,
        type_weight: tw,
        watch_score: 0.0,
        age_credit: 0.0,
        trending_score: 0.0,
        trending_source: None,
        trending_topic_id: None,
        ranking_score: base_score(quality, tw, intrinsic, 0.0, 0.0, card.created_epoch, c),
    }
}

/// `trending_source`: `'listing'` or `'topic'`, NULL otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendingSource {
    Listing,
    Topic,
}

/// The engaged terms of one entity, as the score table holds them.
#[derive(Debug, Clone, PartialEq)]
pub struct Engagement {
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub participation_score: f64,
    pub comment_score: f64,
    pub topic_score: f64,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub trending_source: Option<TrendingSource>,
    pub trending_topic_id: Option<Uuid>,
}

impl Engagement {
    /// True when the row differs from the un-engaged row of 9.1 in an engagement term: the
    /// predicate for "write a put" (false: write a delete).
    pub fn is_engaged(&self) -> bool {
        self.positive != 0
            || self.negative != 0
            || self.stance_positive != 0
            || self.stance_negative != 0
            || self.commenter_count != 0
            || self.participation_score != 0.0
            || self.comment_score != 0.0
            || self.topic_score != 0.0
            || self.watch_score != 0.0
            || self.age_credit != 0.0
            || self.trending_score != 0.0
            || self.trending_source.is_some()
    }
}

/// The value of an overlay row in collection 21: 62 of the 64 bytes geo-db's derived class
/// allows, so no change to geo-db is needed. The row's score (the float8 key the feed orders by)
/// is the stored `ranking_score` in full, topic term included.
///
/// ```text
///  0      version (1)
///  1      trending_source: 0 none, 1 listing, 2 topic
///  2..22  positive, negative, stance_positive, stance_negative, commenter_count (u32 LE each,
///         saturating; the columns are bigint counts that stay far below 2^32)
/// 22..46  watch_score, age_credit, trending_score (f64 LE)
/// 46..62  trending_topic_id (16 bytes, zeros when none)
/// ```
///
/// Not stored, because the reader recomputes them exactly from the above and config:
/// `quality_score` (votes), `participation_score` and `comment_score` (counts), and
/// `topic_score` (not a Entity column; it is inside the key score). `intrinsic_score` and
/// `type_weight` come from the card, as for the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    pub score: f64,
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub trending_source: Option<TrendingSource>,
    pub trending_topic_id: Option<Uuid>,
}

pub const OVERLAY_VERSION: u8 = 1;
pub const OVERLAY_LEN: usize = 62;

fn finite(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

fn sat(n: i64) -> u32 {
    n.clamp(0, u32::MAX as i64) as u32
}

impl Overlay {
    /// The overlay for a stored row: `ranking_score` and its engagement terms.
    pub fn from_row(ranking_score: f64, e: &Engagement) -> Self {
        Overlay {
            score: finite(ranking_score),
            positive: e.positive,
            negative: e.negative,
            stance_positive: e.stance_positive,
            stance_negative: e.stance_negative,
            commenter_count: e.commenter_count,
            watch_score: finite(e.watch_score),
            age_credit: finite(e.age_credit),
            trending_score: finite(e.trending_score),
            trending_source: e.trending_source,
            trending_topic_id: e.trending_topic_id,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(OVERLAY_LEN);
        v.push(OVERLAY_VERSION);
        v.push(match self.trending_source {
            None => 0,
            Some(TrendingSource::Listing) => 1,
            Some(TrendingSource::Topic) => 2,
        });
        for n in [
            self.positive,
            self.negative,
            self.stance_positive,
            self.stance_negative,
            self.commenter_count,
        ] {
            v.extend_from_slice(&sat(n).to_le_bytes());
        }
        for f in [self.watch_score, self.age_credit, self.trending_score] {
            v.extend_from_slice(&f.to_le_bytes());
        }
        v.extend_from_slice(self.trending_topic_id.unwrap_or_default().as_bytes());
        debug_assert_eq!(v.len(), OVERLAY_LEN);
        v
    }

    /// Decodes a stored value; `None` for a wrong length or version.
    pub fn decode(score: f64, v: &[u8]) -> Option<Self> {
        if v.len() != OVERLAY_LEN || v[0] != OVERLAY_VERSION {
            return None;
        }
        let u = |at: usize| u32::from_le_bytes(v[at..at + 4].try_into().unwrap()) as i64;
        let f = |at: usize| f64::from_le_bytes(v[at..at + 8].try_into().unwrap());
        let trending_source = match v[1] {
            0 => None,
            1 => Some(TrendingSource::Listing),
            2 => Some(TrendingSource::Topic),
            _ => return None,
        };
        let topic = Uuid::from_slice(&v[46..62]).ok().filter(|u| !u.is_nil());
        Some(Overlay {
            score,
            positive: u(2),
            negative: u(6),
            stance_positive: u(10),
            stance_negative: u(14),
            commenter_count: u(18),
            watch_score: f(22),
            age_credit: f(30),
            trending_score: f(38),
            trending_source,
            trending_topic_id: topic,
        })
    }

    /// The row for an entity that has an overlay: engagement terms from the overlay, card terms
    /// from the card, quality, participation and comment recomputed from the counts.
    pub fn columns(&self, card: &Card, c: &Config) -> Columns {
        Columns {
            positive: self.positive,
            negative: self.negative,
            stance_positive: self.stance_positive,
            stance_negative: self.stance_negative,
            commenter_count: self.commenter_count,
            quality_score: wilson_lower_bound(self.positive, self.negative, c),
            intrinsic_score: intrinsic_score(card.property_count, card.relation_count, c),
            participation_score: participation_score(
                self.stance_positive + self.stance_negative + self.positive + self.negative,
                c.participation_weight,
                c.participation_cap,
            ),
            comment_score: participation_score(
                self.commenter_count,
                c.comment_weight,
                c.comment_cap,
            ),
            type_weight: type_weight(card),
            watch_score: self.watch_score,
            age_credit: self.age_credit,
            trending_score: self.trending_score,
            trending_source: self.trending_source,
            trending_topic_id: self.trending_topic_id,
            ranking_score: self.score,
        }
    }
}

/// What a reader does for one entity: the overlay row if there is one, else the default.
pub fn read(card: &Card, overlay: Option<&Overlay>, c: &Config) -> Columns {
    match overlay {
        Some(o) => o.columns(card, c),
        None => default_row(card, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity_scores_fixture::SQL_FIXTURE;

    const DEFAULTS: Config = Config {
        tau_seconds: 45000.0,
        wilson_z: 1.96,
        prior_positive: 1.0,
        prior_negative: 1.0,
        quality_floor: 0.01,
        intrinsic_cap: 0.5,
        intrinsic_property_target: 10,
        intrinsic_relation_target: 5,
        participation_weight: 0.0,
        participation_cap: 10.0,
        comment_weight: 0.0,
        comment_cap: 30.0,
    };
    const TUNED: Config = Config {
        tau_seconds: 100000.0,
        participation_weight: 2.5,
        participation_cap: 12.0,
        comment_weight: 1.5,
        ..DEFAULTS
    };

    fn config(n: &str) -> Config {
        match n {
            "defaults" => DEFAULTS,
            "tuned" => TUNED,
            _ => panic!("config {n}"),
        }
    }

    fn card(n: &str, tw: f64) -> Card {
        let (property_count, relation_count, created_epoch) = match n {
            "bare" => (0, 0, Some(1_700_000_000)),
            "mid" => (4, 2, Some(1_760_000_000)),
            "sat" => (25, 9, Some(1_790_000_000)),
            "nocreated" => (3, 1, None),
            _ => panic!("card {n}"),
        };
        Card {
            property_count,
            relation_count,
            created_epoch,
            type_weights: vec![tw],
        }
    }

    /// (positive, negative, stance_positive, stance_negative, commenters, topic, watch, age, trending)
    fn eng(n: &str) -> (i64, i64, i64, i64, i64, f64, f64, f64, f64) {
        match n {
            "none" => (0, 0, 0, 0, 0, 0.0, 0.0, 0.0, 0.0),
            "votes" => (9, 2, 0, 0, 0, 0.0, 0.0, 0.0, 0.0),
            "stance" => (0, 0, 5, 3, 0, 0.0, 0.0, 0.0, 0.0),
            "comments" => (0, 0, 0, 0, 4, 0.0, 0.0, 0.0, 0.0),
            "mixed" => (30, 40, 7, 1, 3, 0.75, 0.5, 1.25, 2.5),
            _ => panic!("eng {n}"),
        }
    }

    fn engagement(n: &str, c: &Config) -> Engagement {
        let (p, ng, sp, sn, cm, topic, watch, age, trend) = eng(n);
        Engagement {
            positive: p,
            negative: ng,
            stance_positive: sp,
            stance_negative: sn,
            commenter_count: cm,
            participation_score: participation_score(
                p + ng + sp + sn,
                c.participation_weight,
                c.participation_cap,
            ),
            comment_score: participation_score(cm, c.comment_weight, c.comment_cap),
            topic_score: topic,
            watch_score: watch,
            age_credit: age,
            trending_score: trend,
            trending_source: if trend != 0.0 {
                Some(TrendingSource::Listing)
            } else {
                None
            },
            trending_topic_id: None,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn the_pure_functions_match_the_sql_on_every_fixture_row() {
        assert_eq!(SQL_FIXTURE.len(), 80);
        let mut weights = std::collections::BTreeSet::new();
        for &(cfg, cd, en, tw, q, i, part, comm, ranking) in SQL_FIXTURE {
            let c = config(cfg);
            let card = card(cd, tw);
            let e = engagement(en, &c);
            let label = format!("{cfg}/{cd}/{en}/{tw}");
            weights.insert((tw * 10.0) as i64);
            assert!(
                close(wilson_lower_bound(e.positive, e.negative, &c), q),
                "quality {label}"
            );
            assert!(
                close(
                    intrinsic_score(card.property_count, card.relation_count, &c),
                    i
                ),
                "intrinsic {label}"
            );
            assert!(close(e.participation_score, part), "participation {label}");
            assert!(close(e.comment_score, comm), "comment {label}");
            let full = base_score(q, tw, i, part, comm, card.created_epoch, &c)
                + e.topic_score
                + e.watch_score
                + e.age_credit
                + e.trending_score;
            assert!(close(full, ranking), "ranking {label}: {full} vs {ranking}");
        }
        assert_eq!(weights.len(), 2, "both type weights are exercised");
    }

    #[test]
    fn the_default_row_is_the_sql_row_of_an_un_engaged_entity() {
        let mut n = 0;
        for &(cfg, cd, en, tw, q, i, _, _, ranking) in SQL_FIXTURE {
            if en != "none" {
                continue;
            }
            let c = config(cfg);
            let d = default_row(&card(cd, tw), &c);
            assert!(close(d.ranking_score, ranking), "{cfg}/{cd}/{tw}");
            assert!(close(d.quality_score, q) && close(d.intrinsic_score, i));
            assert_eq!(d.type_weight, tw);
            assert_eq!(
                (
                    d.positive,
                    d.negative,
                    d.stance_positive,
                    d.stance_negative,
                    d.commenter_count
                ),
                (0, 0, 0, 0, 0)
            );
            assert_eq!(
                (
                    d.participation_score,
                    d.comment_score,
                    d.watch_score,
                    d.age_credit,
                    d.trending_score
                ),
                (0.0, 0.0, 0.0, 0.0, 0.0)
            );
            assert_eq!((d.trending_source, d.trending_topic_id), (None, None));
            n += 1;
        }
        assert_eq!(n, 16);
    }

    #[test]
    fn a_card_with_no_weighted_type_weighs_one_and_the_heaviest_wins() {
        let mut c = card("mid", 1.0);
        c.type_weights.clear();
        assert_eq!(type_weight(&c), 1.0);
        c.type_weights = vec![0.5, 2.5, 1.5];
        assert_eq!(type_weight(&c), 2.5);
    }

    #[test]
    fn only_an_engaged_row_is_stored_and_the_engaged_predicate_is_the_negation_of_the_default() {
        for en in ["none", "votes", "stance", "comments", "mixed"] {
            assert_eq!(engagement(en, &TUNED).is_engaged(), en != "none", "{en}");
        }
        // Each single term alone is enough.
        let base = engagement("none", &TUNED);
        for f in [
            |e: &mut Engagement| e.positive = 1,
            |e: &mut Engagement| e.negative = 1,
            |e: &mut Engagement| e.stance_positive = 1,
            |e: &mut Engagement| e.stance_negative = 1,
            |e: &mut Engagement| e.commenter_count = 1,
            |e: &mut Engagement| e.participation_score = 0.1,
            |e: &mut Engagement| e.comment_score = 0.1,
            |e: &mut Engagement| e.topic_score = 0.1,
            |e: &mut Engagement| e.watch_score = 0.1,
            |e: &mut Engagement| e.age_credit = 0.1,
            |e: &mut Engagement| e.trending_score = 0.1,
            |e: &mut Engagement| e.trending_source = Some(TrendingSource::Topic),
        ] {
            let mut e = base.clone();
            f(&mut e);
            assert!(e.is_engaged());
        }
    }

    #[test]
    fn an_overlay_value_is_62_bytes_and_round_trips() {
        let id = Uuid::from_bytes([7; 16]);
        let mut e = engagement("mixed", &TUNED);
        e.trending_source = Some(TrendingSource::Topic);
        e.trending_topic_id = Some(id);
        let o = Overlay::from_row(42.5, &e);
        let v = o.encode();
        assert_eq!(v.len(), OVERLAY_LEN);
        assert_eq!(Overlay::decode(42.5, &v), Some(o));
        let none = Overlay::from_row(1.0, &engagement("votes", &TUNED));
        assert_eq!(
            Overlay::decode(1.0, &none.encode())
                .unwrap()
                .trending_topic_id,
            None
        );
        assert_eq!(Overlay::decode(1.0, &v[..61]), None);
        let mut bad = v.clone();
        bad[0] = 9;
        assert_eq!(Overlay::decode(1.0, &bad), None);
        let mut bad = v;
        bad[1] = 7;
        assert_eq!(Overlay::decode(1.0, &bad), None);
    }

    #[test]
    fn counts_saturate_rather_than_wrap() {
        let mut e = engagement("votes", &TUNED);
        e.positive = i64::MAX;
        let o = Overlay::decode(1.0, &Overlay::from_row(1.0, &e).encode()).unwrap();
        assert_eq!(o.positive, u32::MAX as i64);
    }

    /// Overlay plus default reproduces the stored order: the stored `ranking_score` of every
    /// fixture entity (SQL, tuned config) against what a reader returns when only the engaged
    /// entities have an overlay row.
    #[test]
    fn overlay_plus_default_reproduces_the_stored_order() {
        let c = TUNED;
        let mut stored: Vec<(String, f64, Columns)> = vec![];
        let mut overlay: std::collections::BTreeMap<String, Overlay> = Default::default();
        let mut cards: std::collections::BTreeMap<String, Card> = Default::default();
        for &(cfg, cd, en, tw, q, _, part, comm, ranking) in SQL_FIXTURE {
            if cfg != "tuned" {
                continue;
            }
            let id = format!("{cd}/{en}/{tw}");
            let e = engagement(en, &c);
            cards.insert(id.clone(), card(cd, tw));
            if e.is_engaged() {
                overlay.insert(id.clone(), Overlay::from_row(ranking, &e));
            }
            // The stored row is exactly what SQL wrote: the full score and the SQL's own terms.
            let cols = Columns {
                quality_score: q,
                participation_score: part,
                comment_score: comm,
                ranking_score: ranking,
                ..default_row(&card(cd, tw), &c)
            };
            stored.push((id, ranking, cols));
        }
        assert_eq!(stored.len(), 40);
        assert_eq!(overlay.len(), 32, "only engaged entities are stored");
        let mut by_stored: Vec<&String> = stored.iter().map(|s| &s.0).collect();
        by_stored.sort_by(|a, b| {
            let (x, y) = (
                stored.iter().find(|s| &s.0 == *a).unwrap().1,
                stored.iter().find(|s| &s.0 == *b).unwrap().1,
            );
            y.partial_cmp(&x).unwrap().then(a.cmp(b))
        });
        let read_all: Vec<(String, Columns)> = stored
            .iter()
            .map(|s| (s.0.clone(), read(&cards[&s.0], overlay.get(&s.0), &c)))
            .collect();
        let mut by_read: Vec<&(String, Columns)> = read_all.iter().collect();
        by_read.sort_by(|a, b| {
            b.1.ranking_score
                .partial_cmp(&a.1.ranking_score)
                .unwrap()
                .then(a.0.cmp(&b.0))
        });
        // Exact ties are broken by id on both sides; near-ties that the default's f64 arithmetic
        // may swap are compared by score, which must agree to 1e-9.
        for (i, (id, cols)) in by_read.iter().enumerate() {
            let s = stored.iter().find(|s| &s.0 == id).unwrap();
            assert!(close(cols.ranking_score, s.1), "{id}");
            assert!(close(cols.quality_score, s.2.quality_score), "quality {id}");
            assert!(
                close(cols.participation_score, s.2.participation_score),
                "participation {id}"
            );
            assert!(close(cols.comment_score, s.2.comment_score), "comment {id}");
            let want = stored.iter().find(|x| x.0 == *by_stored[i]).unwrap().1;
            assert!(close(cols.ranking_score, want), "position {i}");
        }
    }
}
