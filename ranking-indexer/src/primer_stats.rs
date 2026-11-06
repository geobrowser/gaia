//! The Rust twin of `refresh_primer_claim_stats()` (migration 0103, GEO-3143).
//!
//! The SQL rebuilds `primer_claim_stats` from `relations` (which entities are Claims, their topics),
//! `user_votes`, `account_weights` and `values` (the name length), keeping a claim only when
//! `entity_topic_ranking_eligible` holds. [`compute`] does the arithmetic in code from those
//! inputs, so the table can be written to geo-db without the plpgsql function (zero-Postgres plan
//! 10.22 item 6; the target stays the derived class, collections 17-19).
//!
//! Eligibility and the name length are inputs, not recomputed: eligibility is a table-bound helper
//! of its own, and the name length is a `max(length(btrim(text)))` the query supplies.
//!
//! `tests/fixtures/primer_stats_golden.txt` is the SQL's own output on a scratch postgres:16 (70
//! claims, 60 voters, 4378 votes of every kind, weights including 0 and missing, votes after
//! `as_of`, repeat votes, ineligible claims) for three parameter sets. Sums add the weights in
//! user-id order while postgres adds in scan order, so floating results are compared to 1e-9
//! relative; every count, flag, array and the timestamp are compared exactly.
//! Two votes by one user on one claim at the same microsecond are undefined in the SQL
//! (`DISTINCT ON` picks any), and are here too: the grid has none.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::error::IndexerError;
use crate::primer_claims_mirror::ClaimStats;
use crate::sql_helpers::binary_entropy_bits;

/// One `user_votes` row.
#[derive(Debug, Clone, PartialEq)]
pub struct Vote {
    pub user_id: Uuid,
    pub object_id: Uuid,
    pub vote_kind: i16,
    pub vote_type: i16,
    pub voted_us: i64,
}

/// Everything the SQL reads.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// `(claim, space)` for every `Types -> Claim` relation (duplicates allowed).
    pub claim_spaces: Vec<(Uuid, Uuid)>,
    /// `(claim, topic)` for every Topics relation.
    pub claim_topics: Vec<(Uuid, Uuid)>,
    /// `max(length(btrim(name)))` per claim, absent for none.
    pub name_length: HashMap<Uuid, i32>,
    /// Claims for which `entity_topic_ranking_eligible` is false.
    pub ineligible: HashSet<Uuid>,
    /// `account_weights.weight` by user.
    pub weights: HashMap<Uuid, f64>,
    pub votes: Vec<Vote>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub min_voters: f64,
    pub contested_low: f64,
    pub contested_high: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            min_voters: 3.0,
            contested_low: 0.25,
            contested_high: 0.75,
        }
    }
}

/// The statistics rows as of `as_of_us`, ordered by claim id.
pub fn compute(inp: &Inputs, as_of_us: i64, p: &Params) -> Result<Vec<ClaimStats>, IndexerError> {
    if p.min_voters < 0.0
        || p.contested_low < 0.0
        || p.contested_high > 1.0
        || p.contested_low > p.contested_high
        || p.min_voters.is_nan()
        || p.contested_low.is_nan()
        || p.contested_high.is_nan()
    {
        return Err(IndexerError::Config(
            "primer claim stats: invalid parameters".into(),
        ));
    }
    let sorted_unique = |mut v: Vec<Uuid>| {
        v.sort();
        v.dedup();
        v
    };
    let mut spaces: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (c, s) in &inp.claim_spaces {
        spaces.entry(*c).or_default().push(*s);
    }
    let mut topics: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (c, t) in &inp.claim_topics {
        topics.entry(*c).or_default().push(*t);
    }
    let weight = |u: &Uuid| inp.weights.get(u).copied().unwrap_or(0.0);

    // A user's stance on a claim is their latest stance vote on it, in any space.
    let mut latest: HashMap<(Uuid, Uuid), &Vote> = HashMap::new();
    for v in &inp.votes {
        if v.vote_kind == 1
            && (v.vote_type == 0 || v.vote_type == 1)
            && v.voted_us <= as_of_us
            && spaces.contains_key(&v.object_id)
        {
            let slot = latest.entry((v.user_id, v.object_id)).or_insert(v);
            if v.voted_us > slot.voted_us {
                *slot = v;
            }
        }
    }
    // Sums add in user-id order so a run is repeatable.
    let mut stances: Vec<(&(Uuid, Uuid), &&Vote)> = latest.iter().collect();
    stances.sort_by_key(|(k, _)| **k);
    #[derive(Default)]
    struct Tot {
        voters: i32,
        wv: f64,
        wa: f64,
    }
    let mut totals: HashMap<Uuid, Tot> = HashMap::new();
    for ((user, claim), v) in stances {
        let t = totals.entry(*claim).or_default();
        t.voters += 1;
        t.wv += weight(user);
        if v.vote_type == 0 {
            t.wa += weight(user);
        }
    }
    // Engagement: weighted distinct voters of any kind, on claims that have stances.
    let mut engaged: Vec<(Uuid, Uuid)> = inp
        .votes
        .iter()
        .filter(|v| v.voted_us <= as_of_us && totals.contains_key(&v.object_id))
        .map(|v| (v.user_id, v.object_id))
        .collect();
    engaged.sort();
    engaged.dedup();
    let mut engagement: HashMap<Uuid, f64> = HashMap::new();
    for (user, claim) in engaged {
        *engagement.entry(claim).or_insert(0.0) += weight(&user);
    }

    let mut out: Vec<ClaimStats> = Vec::new();
    for (claim, t) in &totals {
        if t.wv <= 0.0 || inp.ineligible.contains(claim) {
            continue;
        }
        let agree_share = t.wa / t.wv;
        let information = binary_entropy_bits(Some((t.wa + 1.0) / (t.wv + 2.0))).unwrap_or(0.0);
        out.push(ClaimStats {
            claim_id: *claim,
            space_ids: sorted_unique(spaces[claim].clone()),
            topic_ids: sorted_unique(topics.get(claim).cloned().unwrap_or_default()),
            voters: t.voters,
            weighted_voters: t.wv,
            weighted_agree: t.wa,
            agree_share,
            information,
            score: information * (1.0 + t.wv).ln(),
            contested: t.wv >= p.min_voters
                && agree_share >= p.contested_low
                && agree_share <= p.contested_high,
            engagement: engagement.get(claim).copied().unwrap_or(t.wv),
            name_length: inp.name_length.get(claim).copied().unwrap_or(0),
            computed_us: as_of_us,
        });
    }
    out.sort_by_key(|r| r.claim_id);
    Ok(out)
}

/// How many rows differ between `rust` and `table` on any mirrored column (floats to 1e-9
/// relative), counting a row on one side only. The shadow check before the SQL is retired.
pub fn count_mismatches(rust: &[ClaimStats], table: &[ClaimStats]) -> usize {
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
    let by: HashMap<Uuid, &ClaimStats> = table.iter().map(|r| (r.claim_id, r)).collect();
    let differing = rust
        .iter()
        .filter(|r| {
            by.get(&r.claim_id).is_none_or(|t| {
                !(t.space_ids == r.space_ids
                    && t.topic_ids == r.topic_ids
                    && t.voters == r.voters
                    && t.contested == r.contested
                    && t.name_length == r.name_length
                    && t.computed_us == r.computed_us
                    && close(t.weighted_voters, r.weighted_voters)
                    && close(t.weighted_agree, r.weighted_agree)
                    && close(t.agree_share, r.agree_share)
                    && close(t.information, r.information)
                    && close(t.score, r.score)
                    && close(t.engagement, r.engagement))
            })
        })
        .count();
    let have: HashSet<Uuid> = rust.iter().map(|r| r.claim_id).collect();
    differing + table.iter().filter(|t| !have.contains(&t.claim_id)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/primer_stats_golden.txt");
    /// 2026-11-06 12:00:00+00.
    const AS_OF_US: i64 = 1_793_966_400_000_000;

    fn ids(s: &str) -> Vec<Uuid> {
        s.split(',')
            .filter(|x| !x.is_empty())
            .map(|x| x.parse().unwrap())
            .collect()
    }

    /// The inputs, and per parameter set the expected rows.
    fn load() -> (Inputs, Vec<(Params, Vec<ClaimStats>)>) {
        let mut inp = Inputs::default();
        let mut runs: Vec<(Params, Vec<ClaimStats>)> = Vec::new();
        for line in GOLDEN.lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f[0] {
                "W" => {
                    inp.weights
                        .insert(f[1].parse().unwrap(), f[2].parse().unwrap());
                }
                "K" => inp
                    .claim_spaces
                    .push((f[1].parse().unwrap(), f[2].parse().unwrap())),
                "T" => inp
                    .claim_topics
                    .push((f[1].parse().unwrap(), f[2].parse().unwrap())),
                "N" => {
                    inp.name_length
                        .insert(f[1].parse().unwrap(), f[2].parse().unwrap());
                }
                "E" => {
                    if f[2] == "1" {
                        inp.ineligible.insert(f[1].parse().unwrap());
                    }
                }
                "V" => inp.votes.push(Vote {
                    user_id: f[1].parse().unwrap(),
                    object_id: f[2].parse().unwrap(),
                    vote_kind: f[3].parse().unwrap(),
                    vote_type: f[4].parse().unwrap(),
                    voted_us: f[5].parse().unwrap(),
                }),
                "P" => runs.push((
                    Params {
                        min_voters: f[1].parse().unwrap(),
                        contested_low: f[2].parse().unwrap(),
                        contested_high: f[3].parse().unwrap(),
                    },
                    Vec::new(),
                )),
                "O" => runs.last_mut().unwrap().1.push(ClaimStats {
                    claim_id: f[1].parse().unwrap(),
                    space_ids: ids(f[2]),
                    topic_ids: ids(f[3]),
                    voters: f[4].parse().unwrap(),
                    weighted_voters: f[5].parse().unwrap(),
                    weighted_agree: f[6].parse().unwrap(),
                    agree_share: f[7].parse().unwrap(),
                    information: f[8].parse().unwrap(),
                    score: f[9].parse().unwrap(),
                    contested: f[10] == "1",
                    engagement: f[11].parse().unwrap(),
                    name_length: f[12].parse().unwrap(),
                    computed_us: f[13].parse().unwrap(),
                }),
                _ => {}
            }
        }
        (inp, runs)
    }

    #[test]
    fn matches_the_sql_over_the_grid() {
        let (inp, runs) = load();
        assert_eq!(runs.len(), 3, "the golden grid is missing");
        let mut contested = (false, false);
        for (params, want) in &runs {
            assert!(want.len() > 50, "{params:?}: {}", want.len());
            let got = compute(&inp, AS_OF_US, params).unwrap();
            assert_eq!(got.len(), want.len(), "{params:?}: one row per claim");
            assert_eq!(count_mismatches(&got, want), 0, "{params:?}");
            for (g, w) in got.iter().zip(want) {
                assert_eq!(g.claim_id, w.claim_id);
                assert_eq!(g.contested, w.contested);
                contested = (contested.0 | w.contested, contested.1 | !w.contested);
            }
        }
        assert_eq!(contested, (true, true), "the grid should hold both kinds");
    }

    #[test]
    fn the_comparison_notices_a_changed_column() {
        let (_, runs) = load();
        let mut table = runs[0].1.clone();
        let rust = table.clone();
        assert_eq!(count_mismatches(&rust, &table), 0);
        table[0].score += 0.01;
        table[1].voters += 1;
        table.pop();
        assert_eq!(count_mismatches(&rust, &table), 3);
    }

    #[test]
    fn invalid_parameters_are_refused_like_the_sql() {
        let ok = Params::default();
        for bad in [
            Params {
                min_voters: -1.0,
                ..ok
            },
            Params {
                contested_low: -0.1,
                ..ok
            },
            Params {
                contested_high: 1.1,
                ..ok
            },
            Params {
                contested_low: 0.8,
                contested_high: 0.2,
                ..ok
            },
        ] {
            assert!(compute(&Inputs::default(), 0, &bad).is_err(), "{bad:?}");
        }
        assert!(compute(&Inputs::default(), 0, &ok).unwrap().is_empty());
    }
}
