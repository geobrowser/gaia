//! The Rust twin of `refresh_anchor_claims()` (migration 0103, GEO-3143).
//!
//! The SQL chooses the monthly anchor set from `primer_claim_stats`. The choice is a pure function
//! of those rows, the newest set's `effective_from` and the parameters, so [`select`] does it in
//! code and [`gate`] makes the "new calendar month, and not before the newest set" decision.
//! `tests/fixtures/anchor_select_golden.txt` is the SQL's own output (scratch postgres:16) over
//! four stats tables and six parameter sets, plus eleven gate cases; the tests compare to it.
//! Writing the set to geo-db (the id, `created_at`) is a later cutover step: today the SQL's set
//! stays the source and a run can only compare against it ([`same_members`]).

use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::{DateTime, Datelike, Utc};
use uuid::Uuid;

use crate::error::IndexerError;

/// The columns of `primer_claim_stats` the selection reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub claim_id: Uuid,
    pub topic_ids: Vec<Uuid>,
    pub weighted_voters: f64,
    pub agree_share: f64,
    pub score: f64,
    pub contested: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub set_size: i32,
    pub min_size: i32,
    pub min_voters: f64,
    pub max_per_topic: i32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            set_size: 40,
            min_size: 30,
            min_voters: 5.0,
            max_per_topic: 4,
        }
    }
}

/// One member of the chosen set, rank from 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub rank: i32,
    pub claim_id: Uuid,
    pub score: f64,
    pub weighted_voters: f64,
    pub agree_share: f64,
}

/// `ORDER BY score DESC, weighted_voters DESC, claim_id`.
fn best_first(a: &Candidate, b: &Candidate) -> Ordering {
    b.score
        .total_cmp(&a.score)
        .then(b.weighted_voters.total_cmp(&a.weighted_voters))
        .then(a.claim_id.cmp(&b.claim_id))
}

/// Whether a new set is due: `Ok(false)` when the newest set took effect in the same UTC month as
/// `as_of` and `force` is off (the SQL returns NULL), an error when `as_of` is not after it (the
/// SQL raises 22023), otherwise `Ok(true)`. Times are microseconds since the epoch.
pub fn gate(latest_us: Option<i64>, as_of_us: i64, force: bool) -> Result<bool, IndexerError> {
    let Some(latest) = latest_us else {
        return Ok(true);
    };
    let month = |us: i64| DateTime::<Utc>::from_timestamp_micros(us).map(|t| (t.year(), t.month()));
    if !force && month(latest) == month(as_of_us) {
        return Ok(false);
    }
    if as_of_us <= latest {
        return Err(IndexerError::Config(
            "anchor claims: as_of is not after the newest set".into(),
        ));
    }
    Ok(true)
}

/// Chooses the set from `stats`, best first (the SQL's three passes). Errors on invalid
/// parameters like the SQL does.
pub fn select(stats: &[Candidate], p: &Params) -> Result<Vec<Member>, IndexerError> {
    if p.set_size < 1
        || p.min_size < 0
        || p.min_size > p.set_size
        || p.min_voters < 0.0
        || p.min_voters.is_nan()
        || p.max_per_topic < 1
    {
        return Err(IndexerError::Config(
            "anchor claims: invalid parameters".into(),
        ));
    }
    let mut sorted: Vec<&Candidate> = stats.iter().collect();
    sorted.sort_by(|a, b| best_first(a, b));
    let eligible = |c: &Candidate| c.weighted_voters >= p.min_voters;
    let (set_size, min_size) = (p.set_size as usize, p.min_size as usize);

    let mut chosen: Vec<&Candidate> = Vec::new();
    let mut taken: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    let mut topic_counts: HashMap<Uuid, i32> = HashMap::new();

    // 1. Contested, topic-capped.
    for c in sorted.iter().filter(|c| c.contested && eligible(c)) {
        if chosen.len() >= set_size {
            break;
        }
        let blocked = c
            .topic_ids
            .iter()
            .any(|t| topic_counts.get(t).copied().unwrap_or(0) >= p.max_per_topic);
        if blocked {
            continue;
        }
        chosen.push(c);
        taken.insert(c.claim_id);
        for t in &c.topic_ids {
            *topic_counts.entry(*t).or_insert(0) += 1;
        }
    }
    // 2. Contested, uncapped. 3. Any claim with enough voters.
    for contested_only in [true, false] {
        if chosen.len() >= min_size {
            break;
        }
        let room = min_size - chosen.len();
        let more: Vec<&Candidate> = sorted
            .iter()
            .filter(|c| (!contested_only || c.contested) && eligible(c))
            .filter(|c| !taken.contains(&c.claim_id))
            .take(room)
            .copied()
            .collect();
        for c in more {
            taken.insert(c.claim_id);
            chosen.push(c);
        }
    }
    Ok(chosen
        .iter()
        .enumerate()
        .map(|(i, c)| Member {
            rank: i as i32 + 1,
            claim_id: c.claim_id,
            score: c.score,
            weighted_voters: c.weighted_voters,
            agree_share: c.agree_share,
        })
        .collect())
}

/// Whether two sets hold the same claims in the same order (the shadow check against the SQL).
pub fn same_members(a: &[Member], b: &[Member]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.claim_id == y.claim_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/anchor_select_golden.txt");

    struct Run {
        params: Params,
        want: Vec<Member>,
    }

    fn scenarios() -> Vec<(Vec<Candidate>, Vec<Run>)> {
        let mut out: Vec<(Vec<Candidate>, Vec<Run>)> = Vec::new();
        for line in GOLDEN.lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f[0] {
                "S" => out.push((Vec::new(), Vec::new())),
                "C" => out.last_mut().unwrap().0.push(Candidate {
                    claim_id: f[1].parse().unwrap(),
                    contested: f[2] == "1",
                    weighted_voters: f[3].parse().unwrap(),
                    score: f[4].parse().unwrap(),
                    agree_share: f[5].parse().unwrap(),
                    topic_ids: f[6]
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.parse().unwrap())
                        .collect(),
                }),
                "R" => out.last_mut().unwrap().1.push(Run {
                    params: Params {
                        set_size: f[1].parse().unwrap(),
                        min_size: f[2].parse().unwrap(),
                        min_voters: f[3].parse().unwrap(),
                        max_per_topic: f[4].parse().unwrap(),
                    },
                    want: Vec::new(),
                }),
                "M" => out
                    .last_mut()
                    .unwrap()
                    .1
                    .last_mut()
                    .unwrap()
                    .want
                    .push(Member {
                        rank: f[1].parse().unwrap(),
                        claim_id: f[2].parse().unwrap(),
                        score: f[3].parse().unwrap(),
                        weighted_voters: f[4].parse().unwrap(),
                        agree_share: f[5].parse().unwrap(),
                    }),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn selection_matches_the_sql_over_the_grid() {
        let scenarios = scenarios();
        assert_eq!(scenarios.len(), 4, "the golden grid is missing");
        let mut runs = 0;
        let mut sizes = std::collections::HashSet::new();
        for (stats, run) in &scenarios {
            for r in run {
                let got = select(stats, &r.params).unwrap();
                assert_eq!(got.len(), r.want.len(), "{:?}: size", r.params);
                assert!(!got.is_empty());
                for (g, w) in got.iter().zip(&r.want) {
                    assert_eq!(g.rank, w.rank);
                    assert_eq!(g.claim_id, w.claim_id, "{:?} rank {}", r.params, w.rank);
                    assert_eq!(g.score.to_bits(), w.score.to_bits());
                    assert_eq!(g.weighted_voters.to_bits(), w.weighted_voters.to_bits());
                    assert_eq!(g.agree_share.to_bits(), w.agree_share.to_bits());
                }
                sizes.insert(got.len());
                runs += 1;
            }
        }
        assert_eq!(runs, 24);
        assert!(
            sizes.len() >= 6,
            "the grid should reach several set sizes: {sizes:?}"
        );
    }

    #[test]
    fn the_gate_matches_the_sql() {
        let mut n = 0;
        for line in GOLDEN.lines().filter(|l| l.starts_with("G|")) {
            let f: Vec<&str> = line.split('|').collect();
            let latest = (f[1] != "N").then(|| f[1].parse::<i64>().unwrap());
            let got = gate(latest, f[2].parse().unwrap(), f[3] == "1");
            match f[4] {
                "new" => assert!(matches!(got, Ok(true)), "{line}"),
                "skip" => assert!(matches!(got, Ok(false)), "{line}"),
                "err" => assert!(got.is_err(), "{line}"),
                other => panic!("{other}"),
            }
            n += 1;
        }
        assert_eq!(n, 11);
    }

    #[test]
    fn invalid_parameters_are_refused_like_the_sql() {
        let ok = Params::default();
        for bad in [
            Params { set_size: 0, ..ok },
            Params { min_size: -1, ..ok },
            Params { min_size: 41, ..ok },
            Params {
                min_voters: -1.0,
                ..ok
            },
            Params {
                max_per_topic: 0,
                ..ok
            },
        ] {
            assert!(select(&[], &bad).is_err(), "{bad:?}");
        }
        assert!(select(&[], &ok).unwrap().is_empty());
    }

    #[test]
    fn same_members_compares_claims_in_order() {
        let m = |n: u8| Member {
            rank: 1,
            claim_id: Uuid::from_bytes([n; 16]),
            score: 0.0,
            weighted_voters: 0.0,
            agree_share: 0.0,
        };
        assert!(same_members(&[m(1), m(2)], &[m(1), m(2)]));
        assert!(!same_members(&[m(1), m(2)], &[m(2), m(1)]));
        assert!(!same_members(&[m(1)], &[m(1), m(2)]));
    }
}
