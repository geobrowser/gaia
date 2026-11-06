//! The stance-map plpgsql (GEO-3146, migrations 0108 and 0117) as pure Rust, next to the fit in
//! `stance_map.rs`: the inputs the fit reads and the gate it is judged by. Plan 10.21, family 3.
//!
//! | SQL object | Rust |
//! | -- | -- |
//! | `stance_map_observations` | [`observations`] |
//! | `stance_map_seeds` | [`seeds`] |
//! | `record_stance_map_run` (validate and gate; the run table is no longer written, 0117) | [`record_run`] |
//!
//! Nothing here touches a database. The rows come from the graph statements the ranking service
//! will hold; [`user_interest::Rel`] and [`user_interest::Vote`] are the shared row shapes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use uuid::Uuid;

use crate::user_interest::{epoch_text_us, Rel, Vote, CLAIMS, DEBATE_TYPE, TYPES};

/// The `Claim` type.
pub const CLAIM_TYPE: Uuid = Uuid::from_u128(0x96f859ef_a1ca_4b22_9372_c86ad58b694b);
/// `Supported by`: a debate side that stated Agree.
pub const SUPPORTED_BY: Uuid = Uuid::from_u128(0xd19fad56_5136_4a7f_8309_daf5c7bf99dd);
/// `Opposed by`: a debate side that stated Disagree.
pub const OPPOSED_BY: Uuid = Uuid::from_u128(0xc57de77c_3eee_4e7b_a0d2_258d18aab11c);
/// `Supports`: a claim supporting a main claim.
pub const SUPPORTS: Uuid = Uuid::from_u128(0x81faa4ad_afad_4009_b106_1374c1219f04);
/// `Opposes`: a claim opposing a main claim.
pub const OPPOSES: Uuid = Uuid::from_u128(0x71d1bcd5_f1cf_487f_b9a3_c7b1d53803eb);

/// Where an observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Vote,
    Stated,
}

/// A row of `stance_map_observations`.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub user: Uuid,
    pub claim: Uuid,
    pub agree: bool,
    pub weight: f64,
    pub source: Source,
}

/// What `observations` reads. `entity_created` is `entities.created_at` as stored (epoch seconds
/// as text); an entity absent from the map does not exist.
#[derive(Debug, Default, Clone)]
pub struct StanceInput {
    pub relations: Vec<Rel>,
    pub votes: Vec<Vote>,
    pub account_weights: HashMap<Uuid, f64>,
    pub entity_created: HashMap<Uuid, Option<String>>,
}

/// `stance_map_observations(as_of)`: the votes (latest Agree/Disagree per user and claim, claims
/// only, weighted by account weight, weight 0 left out) followed by the debaters' stated sides
/// (weighted by account weight times `stated_weight`, dropped when that is 0). Sorted by
/// (source, user, claim). Where a user has two votes at the same instant the SQL's pick is
/// arbitrary; here the later row wins.
pub fn observations(g: &StanceInput, as_of_us: i64, stated_weight: f64) -> Vec<Observation> {
    let typed = |e: Uuid, t: Uuid| {
        g.relations
            .iter()
            .any(|r| r.type_id == TYPES && r.from == e && r.to == t)
    };
    let claims: HashSet<Uuid> = g
        .relations
        .iter()
        .filter(|r| r.type_id == TYPES && r.to == CLAIM_TYPE)
        .map(|r| r.from)
        .collect();

    let mut latest: BTreeMap<(Uuid, Uuid), (i64, i16)> = BTreeMap::new();
    for v in &g.votes {
        if v.vote_kind == 1
            && matches!(v.vote_type, 0 | 1)
            && v.voted_at_us <= as_of_us
            && claims.contains(&v.object)
        {
            latest
                .entry((v.user, v.object))
                .and_modify(|c| {
                    if v.voted_at_us >= c.0 {
                        *c = (v.voted_at_us, v.vote_type);
                    }
                })
                .or_insert((v.voted_at_us, v.vote_type));
        }
    }

    // A debate's main claim: exactly one distinct Claims target (the SQL takes min() of one).
    let mut main: BTreeMap<Uuid, BTreeSet<Uuid>> = BTreeMap::new();
    for r in g.relations.iter().filter(|r| r.type_id == CLAIMS) {
        if typed(r.from, DEBATE_TYPE) {
            main.entry(r.from).or_default().insert(r.to);
        }
    }
    let main: BTreeMap<Uuid, Uuid> = main
        .into_iter()
        .filter(|(_, s)| s.len() == 1)
        .map(|(d, s)| (d, *s.iter().next().unwrap()))
        .collect();

    // Distinct (debate, user, side), then per (user, claim) the side argued more often; a tie
    // drops the claim for that user. A debate whose creation time is readable and after as_of is
    // left out; an unreadable or missing time keeps it, but the entity row must exist.
    let sides: BTreeSet<(Uuid, Uuid, bool)> = g
        .relations
        .iter()
        .filter(|r| matches!(r.type_id, SUPPORTED_BY | OPPOSED_BY) && main.contains_key(&r.from))
        .map(|r| (r.from, r.to, r.type_id == SUPPORTED_BY))
        .collect();
    let mut tally: BTreeMap<(Uuid, Uuid), (i64, i64)> = BTreeMap::new();
    for (debate, user, agree) in sides {
        let Some(created) = g.entity_created.get(&debate) else {
            continue;
        };
        let readable = created.as_deref().and_then(epoch_text_us);
        if readable.is_some_and(|t| t > as_of_us) {
            continue;
        }
        let t = tally.entry((user, main[&debate])).or_default();
        if agree {
            t.0 += 1;
        } else {
            t.1 += 1;
        }
    }

    let weight = |u: Uuid| g.account_weights.get(&u).copied().filter(|w| *w > 0.0);
    let mut out: Vec<Observation> = Vec::new();
    for ((user, claim), (_, vote_type)) in latest {
        if let Some(w) = weight(user) {
            out.push(Observation {
                user,
                claim,
                agree: vote_type == 0,
                weight: w,
                source: Source::Vote,
            });
        }
    }
    if stated_weight > 0.0 {
        for ((user, claim), (a, d)) in tally {
            if a == d {
                continue;
            }
            if let Some(w) = weight(user) {
                out.push(Observation {
                    user,
                    claim,
                    agree: a > d,
                    weight: w * stated_weight,
                    source: Source::Stated,
                });
            }
        }
    }
    out.sort_by_key(|a| (a.source, a.user, a.claim));
    out
}

/// A row of `stance_map_seeds`: `sign` is +1 for Supports, -1 for Opposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeedRow {
    pub claim: Uuid,
    pub main_claim: Uuid,
    pub sign: i8,
}

/// `stance_map_seeds()`: a pair with only Supports gets +1, only Opposes -1, both is dropped, a
/// claim relating to itself is dropped. Sorted.
pub fn seeds(relations: &[Rel]) -> Vec<SeedRow> {
    let mut by_pair: BTreeMap<(Uuid, Uuid), BTreeSet<Uuid>> = BTreeMap::new();
    for r in relations {
        if matches!(r.type_id, SUPPORTS | OPPOSES) && r.from != r.to {
            by_pair.entry((r.from, r.to)).or_default().insert(r.type_id);
        }
    }
    by_pair
        .into_iter()
        .filter(|(_, t)| t.len() == 1)
        .map(|((claim, main_claim), t)| SeedRow {
            claim,
            main_claim,
            sign: if t.contains(&SUPPORTS) { 1 } else { -1 },
        })
        .collect()
}

/// A row of `stance_map_config`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateConfig {
    pub min_clean_votes: i64,
    pub min_split_correlation: f64,
    pub min_auc_lift: f64,
    pub stated_weight: f64,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            min_clean_votes: 10_000,
            min_split_correlation: 0.7,
            min_auc_lift: 0.0,
            stated_weight: 1.0,
        }
    }
}

/// The metrics `record_stance_map_run` reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunMetrics {
    pub model_version: Option<String>,
    pub clean_votes: Option<i64>,
    pub split_correlation: Option<f64>,
    pub auc_lift: Option<f64>,
}

/// The conditions a run can fail, named as the run table named them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateFailure {
    CleanVotes,
    SplitCorrelation,
    AucLift,
}

impl GateFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            GateFailure::CleanVotes => "clean_votes",
            GateFailure::SplitCorrelation => "split_correlation",
            GateFailure::AucLift => "auc_lift",
        }
    }
}

/// The result of recording a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunVerdict {
    pub gate_met: bool,
    pub failures: Vec<GateFailure>,
}

/// `record_stance_map_run`: the metrics must name a model version and a clean vote count (SQL
/// error 22023 otherwise); the gate fails on too few clean votes, and on a split correlation or
/// AUC lift that is missing or does not clear its threshold (a run that could not measure
/// stability has not shown it).
pub fn record_run(m: &RunMetrics, cfg: &GateConfig) -> Result<RunVerdict, &'static str> {
    let (Some(_), Some(clean)) = (&m.model_version, m.clean_votes) else {
        return Err(
            "record_stance_map_run: metrics must be an object with model_version and clean_votes",
        );
    };
    let mut failures = Vec::new();
    if clean < cfg.min_clean_votes {
        failures.push(GateFailure::CleanVotes);
    }
    if !m
        .split_correlation
        .is_some_and(|c| c >= cfg.min_split_correlation)
    {
        failures.push(GateFailure::SplitCorrelation);
    }
    if !m.auc_lift.is_some_and(|l| l > cfg.min_auc_lift) {
        failures.push(GateFailure::AucLift);
    }
    Ok(RunVerdict {
        gate_met: failures.is_empty(),
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    fn rel(t: Uuid, from: u128, to: Uuid) -> Rel {
        Rel {
            type_id: t,
            from: id(from),
            to,
            space: id(0),
        }
    }
    fn rel_i(t: Uuid, from: u128, to: u128) -> Rel {
        rel(t, from, id(to))
    }
    fn vote(user: u128, object: u128, ty: i16, at: i64) -> Vote {
        Vote {
            user: id(user),
            object: id(object),
            object_type: 0,
            vote_kind: 1,
            vote_type: ty,
            voted_at_us: at,
        }
    }
    const T: i64 = 1_700_000_000_000_000;

    fn base() -> StanceInput {
        let mut g = StanceInput::default();
        g.relations.push(rel(TYPES, 10, CLAIM_TYPE));
        g.relations.push(rel(TYPES, 11, CLAIM_TYPE));
        g.account_weights.insert(id(1), 1.0);
        g.account_weights.insert(id(2), 0.5);
        g.account_weights.insert(id(3), 0.0);
        g
    }

    #[test]
    fn a_vote_is_the_latest_stance_on_a_claim_weighted_and_zero_weight_is_left_out() {
        let mut g = base();
        g.votes = vec![
            vote(1, 10, 1, T),
            vote(1, 10, 0, T + 5), // later: agree
            vote(2, 11, 1, T),     // disagree, weight .5
            vote(3, 10, 0, T),     // weight 0
            vote(1, 99, 0, T),     // not a claim
            vote(1, 11, 2, T),     // removal
            Vote {
                vote_kind: 3,
                ..vote(1, 11, 0, T)
            }, // not a stance vote
            vote(2, 10, 0, T + 100), // after as_of
        ];
        let o = observations(&g, T + 10, 1.0);
        assert_eq!(o.len(), 2);
        assert_eq!(
            (o[0].user, o[0].claim, o[0].agree, o[0].weight),
            (id(1), id(10), true, 1.0)
        );
        assert_eq!((o[1].user, o[1].agree, o[1].weight), (id(2), false, 0.5));
        assert!(o.iter().all(|x| x.source == Source::Vote));
    }

    fn debate(g: &mut StanceInput, d: u128, claim: u128, created: &str) {
        g.relations.push(rel(TYPES, d, DEBATE_TYPE));
        g.relations.push(rel_i(CLAIMS, d, claim));
        g.entity_created.insert(id(d), Some(created.into()));
    }

    #[test]
    fn a_stated_side_counts_with_the_stated_weight_and_a_tie_states_nothing() {
        let mut g = base();
        debate(&mut g, 20, 10, "1700000000");
        debate(&mut g, 21, 10, "1700000001");
        g.relations.push(rel_i(SUPPORTED_BY, 20, 1));
        g.relations.push(rel_i(OPPOSED_BY, 20, 2));
        // user 2 argues for it in debate 21 as well: 1 for, 1 against = tie, dropped
        g.relations.push(rel_i(SUPPORTED_BY, 21, 2));
        let o = observations(&g, T + 10_000_000, 2.0);
        assert_eq!(o.len(), 1);
        assert_eq!(
            (o[0].user, o[0].agree, o[0].weight, o[0].source),
            (id(1), true, 2.0, Source::Stated)
        );
        // stated_weight 0 drops all stated rows
        assert!(observations(&g, T + 10_000_000, 0.0).is_empty());
    }

    #[test]
    fn the_side_argued_more_often_wins() {
        let mut g = base();
        for d in [20u128, 21, 22] {
            debate(&mut g, d, 10, "1700000000");
        }
        g.relations.push(rel_i(OPPOSED_BY, 20, 1));
        g.relations.push(rel_i(OPPOSED_BY, 21, 1));
        g.relations.push(rel_i(SUPPORTED_BY, 22, 1));
        let o = observations(&g, T + 10, 1.0);
        assert!(!o[0].agree);
    }

    #[test]
    fn a_debate_with_two_main_claims_or_none_states_nothing() {
        let mut g = base();
        debate(&mut g, 20, 10, "1700000000");
        g.relations.push(rel_i(CLAIMS, 20, 11)); // second main claim: ambiguous
        g.relations.push(rel_i(SUPPORTED_BY, 20, 1));
        g.relations.push(rel(TYPES, 21, DEBATE_TYPE)); // topic debate: no Claims
        g.entity_created.insert(id(21), Some("1700000000".into()));
        g.relations.push(rel_i(SUPPORTED_BY, 21, 1));
        assert!(observations(&g, T + 10, 1.0).is_empty());
    }

    #[test]
    fn a_debate_created_after_as_of_is_left_out_but_an_unreadable_time_is_kept() {
        let mut g = base();
        debate(&mut g, 20, 10, "1700001000"); // after as_of
        debate(&mut g, 21, 11, "junk"); // unreadable: kept
        g.relations.push(rel_i(SUPPORTED_BY, 20, 1));
        g.relations.push(rel_i(SUPPORTED_BY, 21, 1));
        let o = observations(&g, T, 1.0);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].claim, id(11));
        // a debate with no entity row is not joined
        g.entity_created.remove(&id(21));
        assert!(observations(&g, T, 1.0).is_empty());
    }

    #[test]
    fn votes_come_before_stated_rows_in_the_output() {
        let mut g = base();
        g.votes = vec![vote(2, 10, 0, T)];
        debate(&mut g, 20, 11, "1700000000");
        g.relations.push(rel_i(SUPPORTED_BY, 20, 1));
        let o = observations(&g, T + 10, 1.0);
        assert_eq!(
            o.iter().map(|x| x.source).collect::<Vec<_>>(),
            vec![Source::Vote, Source::Stated]
        );
    }

    #[test]
    fn seeds_are_signed_and_a_contradicting_or_self_pair_is_dropped() {
        let rels = vec![
            rel_i(SUPPORTS, 1, 9),
            rel_i(SUPPORTS, 1, 9), // the same relation in another space
            rel_i(OPPOSES, 2, 9),
            rel_i(SUPPORTS, 3, 9),
            rel_i(OPPOSES, 3, 9),  // contradicts itself
            rel_i(SUPPORTS, 4, 4), // self
            rel_i(TYPES, 5, 9),    // not a seed
        ];
        let s = seeds(&rels);
        assert_eq!(
            s,
            vec![
                SeedRow {
                    claim: id(1),
                    main_claim: id(9),
                    sign: 1
                },
                SeedRow {
                    claim: id(2),
                    main_claim: id(9),
                    sign: -1
                },
            ]
        );
    }

    fn metrics(clean: i64, split: Option<f64>, lift: Option<f64>) -> RunMetrics {
        RunMetrics {
            model_version: Some("v".into()),
            clean_votes: Some(clean),
            split_correlation: split,
            auc_lift: lift,
        }
    }

    #[test]
    fn the_gate_passes_only_when_all_three_conditions_clear() {
        let c = GateConfig::default();
        assert!(
            record_run(&metrics(10_000, Some(0.7), Some(0.01)), &c)
                .unwrap()
                .gate_met
        );
        let v = record_run(&metrics(9_999, Some(0.69), Some(0.0)), &c).unwrap();
        assert!(!v.gate_met);
        assert_eq!(
            v.failures.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
            vec!["clean_votes", "split_correlation", "auc_lift"]
        );
    }

    #[test]
    fn a_missing_score_fails_its_condition() {
        let c = GateConfig::default();
        let v = record_run(&metrics(20_000, None, None), &c).unwrap();
        assert_eq!(
            v.failures,
            vec![GateFailure::SplitCorrelation, GateFailure::AucLift]
        );
    }

    #[test]
    fn metrics_without_a_version_or_clean_votes_are_refused() {
        let c = GateConfig::default();
        assert!(record_run(&RunMetrics::default(), &c).is_err());
        let mut m = metrics(1, Some(1.0), Some(1.0));
        m.model_version = None;
        assert!(record_run(&m, &c).is_err());
        m.model_version = Some("v".into());
        m.clean_votes = None;
        assert!(record_run(&m, &c).is_err());
    }
}
