//! Rust twins of `public.claim_overlap_share` and `public.anchor_claim_set_at` (migration 0103, GEO-3143),
//! and of the once-a-day decision in `public.record_claim_overlap_sample`.
//!
//! `claim_overlap_share` is the ticket's outcome measure: among users with weight above 0 and at least
//! `min_votes` stance votes on claims cast by `as_of`, the share of user pairs that took a stance on at
//! least `min_shared` of the same claims. It reads `relations` (which entities are claims),
//! `user_votes` and `account_weights`. [`overlap`] does it in code from those rows, so the sample can be
//! written to geo-db without the SQL function (zero-Postgres plan 10.22, groups D/E remainder).
//!
//! Unlike the primer statistics this counts every distinct `(user, claim)` stance, not the latest vote:
//! a stance is any Agree or Disagree vote of kind 1 on the claim by `as_of`.
//!
//! `tests/fixtures/claim_overlap_golden.txt` is the SQL's own output on a scratch postgres:16 (40
//! entities of which 30 are claims, one claim typed twice, 50 users with weights including 0 and
//! missing, 6000 votes of every kind and type over three months, three anchor sets) for six `as_of`
//! instants and eight `(min_votes, min_shared)` pairs. Every field is compared exactly, the share by bits.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::primer_stats::Vote;

/// The result row of `claim_overlap_share`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlap {
    pub users: i32,
    pub pairs: i64,
    pub pairs_sharing: i64,
    /// `None` when fewer than two users qualify (the SQL's NULL).
    pub share: Option<f64>,
}

/// `claim_overlap_share(min_votes, min_shared, as_of)`.
///
/// `claims` is the set of entities with a `Types -> Claim` relation. `weights` is `account_weights`;
/// a user without a row, or with weight 0 or less, is ignored. A `min_shared` of 0 or less behaves as 1,
/// because a pair only exists through a claim both stood on.
pub fn overlap(
    claims: &HashSet<Uuid>,
    weights: &HashMap<Uuid, f64>,
    votes: &[Vote],
    as_of_us: i64,
    min_votes: i32,
    min_shared: i32,
) -> Overlap {
    let mut stances: HashSet<(Uuid, Uuid)> = HashSet::new();
    for v in votes {
        if v.vote_kind == 1
            && (v.vote_type == 0 || v.vote_type == 1)
            && v.voted_us <= as_of_us
            && claims.contains(&v.object_id)
            && weights.get(&v.user_id).is_some_and(|w| *w > 0.0)
        {
            stances.insert((v.user_id, v.object_id));
        }
    }
    let mut per_user: HashMap<Uuid, usize> = HashMap::new();
    for (u, _) in &stances {
        *per_user.entry(*u).or_default() += 1;
    }
    let counted: HashSet<Uuid> = per_user
        .into_iter()
        .filter(|(_, n)| *n as i64 >= min_votes as i64)
        .map(|(u, _)| u)
        .collect();
    // Voters per claim among the counted users; a pair shares a claim when both are in its list.
    let mut by_claim: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (u, c) in &stances {
        if counted.contains(u) {
            by_claim.entry(*c).or_default().push(*u);
        }
    }
    let mut shared: HashMap<(Uuid, Uuid), i64> = HashMap::new();
    for voters in by_claim.values_mut() {
        voters.sort();
        for i in 0..voters.len() {
            for j in i + 1..voters.len() {
                *shared.entry((voters[i], voters[j])).or_default() += 1;
            }
        }
    }
    let sharing = shared.values().filter(|n| **n >= min_shared as i64).count() as i64;
    let n = counted.len() as i64;
    let pairs = n * (n - 1) / 2;
    Overlap {
        users: n as i32,
        pairs,
        pairs_sharing: sharing,
        share: (n > 1).then(|| sharing as f64 / pairs as f64),
    }
}

/// An `anchor_claim_sets` row as the lookup needs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorSet {
    pub id: i32,
    pub effective_us: i64,
}

/// `anchor_claim_set_at(as_of)`: the newest set that took effect at or before `as_of`, or `None`.
/// Ties on `effective_from` are undefined in the SQL; here the larger id wins.
pub fn anchor_claim_set_at(sets: &[AnchorSet], as_of_us: i64) -> Option<i32> {
    sets.iter()
        .filter(|s| s.effective_us <= as_of_us)
        .max_by_key(|s| (s.effective_us, s.id))
        .map(|s| s.id)
}

/// The UTC day of an instant in microseconds, as a day number.
fn utc_day(us: i64) -> i64 {
    us.div_euclid(86_400_000_000)
}

/// `record_claim_overlap_sample`'s guard: it writes nothing when a sample with the same
/// `(min_votes, min_shared)` already exists for the UTC day of `as_of`. `existing` is
/// `(sampled_at_us, min_votes, min_shared)` per stored sample.
pub fn should_record(
    existing: &[(i64, i32, i32)],
    as_of_us: i64,
    min_votes: i32,
    min_shared: i32,
) -> bool {
    !existing.iter().any(|(at, mv, ms)| {
        utc_day(*at) == utc_day(as_of_us) && *mv == min_votes && *ms == min_shared
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/claim_overlap_golden.txt");

    struct Row {
        as_of: i64,
        mv: i32,
        ms: i32,
        want: Overlap,
        set: Option<i32>,
    }

    #[allow(clippy::type_complexity)]
    fn load() -> (
        HashSet<Uuid>,
        HashMap<Uuid, f64>,
        Vec<Vote>,
        Vec<AnchorSet>,
        Vec<Row>,
    ) {
        let (mut claims, mut weights, mut votes, mut sets, mut rows) = (
            HashSet::new(),
            HashMap::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            match f[0] {
                "C" => {
                    claims.insert(f[1].parse().unwrap());
                }
                "W" => {
                    weights.insert(f[1].parse().unwrap(), f[2].parse().unwrap());
                }
                "V" => votes.push(Vote {
                    user_id: f[1].parse().unwrap(),
                    object_id: f[2].parse().unwrap(),
                    vote_kind: f[3].parse().unwrap(),
                    vote_type: f[4].parse().unwrap(),
                    voted_us: f[5].parse().unwrap(),
                }),
                "S" => sets.push(AnchorSet {
                    id: f[1].parse().unwrap(),
                    effective_us: f[2].parse().unwrap(),
                }),
                "R" => rows.push(Row {
                    as_of: f[1].parse().unwrap(),
                    mv: f[2].parse().unwrap(),
                    ms: f[3].parse().unwrap(),
                    want: Overlap {
                        users: f[4].parse().unwrap(),
                        pairs: f[5].parse().unwrap(),
                        pairs_sharing: f[6].parse().unwrap(),
                        share: (f[7] != "NULL").then(|| f[7].parse().unwrap()),
                    },
                    set: (f[8] != "NULL").then(|| f[8].parse().unwrap()),
                }),
                t => panic!("tag {t}"),
            }
        }
        (claims, weights, votes, sets, rows)
    }

    #[test]
    fn overlap_and_anchor_set_match_sql() {
        let (claims, weights, votes, sets, rows) = load();
        assert_eq!(
            (claims.len(), weights.len(), votes.len(), sets.len()),
            (30, 45, 6000, 3)
        );
        assert_eq!(rows.len(), 48);
        for r in &rows {
            let got = overlap(&claims, &weights, &votes, r.as_of, r.mv, r.ms);
            assert_eq!(
                got.users, r.want.users,
                "users at {} {} {}",
                r.as_of, r.mv, r.ms
            );
            assert_eq!(got.pairs, r.want.pairs);
            assert_eq!(
                got.pairs_sharing, r.want.pairs_sharing,
                "{} {} {}",
                r.as_of, r.mv, r.ms
            );
            assert_eq!(got.share.map(f64::to_bits), r.want.share.map(f64::to_bits));
            assert_eq!(anchor_claim_set_at(&sets, r.as_of), r.set);
        }
        // The grid covers an empty result, a null share, and a share strictly between 0 and 1.
        assert!(rows.iter().any(|r| r.want.share.is_none()));
        assert!(rows
            .iter()
            .any(|r| r.want.share.is_some_and(|s| s > 0.0 && s < 1.0)));
        assert!(rows.iter().any(|r| r.set.is_none()) && rows.iter().any(|r| r.set == Some(3)));
    }

    #[test]
    fn once_a_day_guard() {
        let day = 86_400_000_000i64;
        let t = 20_000 * day + 5;
        assert!(should_record(&[], t, 5, 3));
        assert!(!should_record(&[(20_000 * day, 5, 3)], t, 5, 3));
        assert!(should_record(&[(20_000 * day, 5, 4)], t, 5, 3));
        assert!(should_record(&[(19_999 * day + day - 1, 5, 3)], t, 5, 3));
        assert!(!should_record(&[(20_000 * day + day - 1, 5, 3)], t, 5, 3));
    }
}
