//! The Rust twin of `refresh_account_weights()` (migration 0101, GEO-3141).
//!
//! The SQL aggregates `user_votes` per voter, joins `account_exclusions` and writes one
//! `account_weights` row per voter. [`compute`] does the scoring half in code from the same
//! per-voter aggregates ([`AGG_SQL`]) and exclusions ([`EXCLUSIONS_SQL`]), so the weights can be
//! written to geo-db without the plpgsql function (zero-Postgres, plan 10.22 item 6).
//!
//! Only what the geo-db row carries is computed: the weight, the counts, the first vote and the
//! reason bits. The reasons' `detail` text is not mirrored (see `account_weights_mirror`).
//! `tests/fixtures/account_weights_golden.txt` is the SQL's own output over a grid of voters and
//! two parameter sets, run on a scratch postgres:16, and the tests compare to it bit for bit.

use std::collections::HashSet;

use uuid::Uuid;

use crate::account_weights_mirror::{
    AccountWeight, REASON_EXCLUDED, REASON_NEW_ACCOUNT, REASON_ONE_SIDED,
};
use crate::error::IndexerError;

/// One voter's votes, aggregated the way the SQL's `voters` CTE does.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct VoterAgg {
    pub user_id: Uuid,
    pub vote_count: i32,
    /// Votes with `vote_kind = 1` (stance).
    pub stance_votes: i32,
    /// Stance votes with `vote_type = 0` (Agree).
    pub agrees: i32,
    /// `min(voted_at)` in microseconds since the epoch.
    pub first_vote_us: i64,
}

/// The aggregate the SQL's `voters` CTE computes, in the shape [`VoterAgg`] takes.
pub const AGG_SQL: &str = "SELECT user_id, count(*)::int AS vote_count, \
    (count(*) FILTER (WHERE vote_kind = 1))::int AS stance_votes, \
    (count(*) FILTER (WHERE vote_kind = 1 AND vote_type = 0))::int AS agrees, \
    (extract(epoch FROM min(voted_at))::numeric * 1000000)::bigint AS first_vote_us \
    FROM public.user_votes GROUP BY user_id";

/// The accounts that carry weight 0.
pub const EXCLUSIONS_SQL: &str = "SELECT user_id FROM public.account_exclusions";

/// The thresholds `refresh_account_weights()` takes, with its defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub ramp_days: f64,
    pub one_sided_min_votes: i32,
    pub one_sided_share: f64,
    pub one_sided_factor: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            ramp_days: 7.0,
            one_sided_min_votes: 20,
            one_sided_share: 0.95,
            one_sided_factor: 0.25,
        }
    }
}

impl Params {
    /// The SQL's parameter check (it raises 22023).
    pub fn validate(&self) -> Result<(), IndexerError> {
        let bad = self.ramp_days <= 0.0
            || self.one_sided_min_votes < 1
            || self.one_sided_share <= 0.5
            || self.one_sided_share > 1.0
            || self.one_sided_factor < 0.0
            || self.one_sided_factor > 1.0
            || self.ramp_days.is_nan()
            || self.one_sided_share.is_nan()
            || self.one_sided_factor.is_nan();
        if bad {
            return Err(IndexerError::Config(
                "account weights: invalid parameters".into(),
            ));
        }
        Ok(())
    }
}

/// Scores every voter as of `as_of_us` (microseconds since the epoch), in the input order.
pub fn compute(
    voters: &[VoterAgg],
    excluded: &HashSet<Uuid>,
    as_of_us: i64,
    p: &Params,
) -> Result<Vec<AccountWeight>, IndexerError> {
    p.validate()?;
    Ok(voters
        .iter()
        .map(|v| {
            let is_excluded = excluded.contains(&v.user_id);
            // extract(epoch FROM interval) is exact microseconds as a numeric; the division of an
            // exact integer by 1e6 rounds the same way the numeric to double cast does.
            let elapsed_s = (as_of_us - v.first_vote_us) as f64 / 1e6;
            let ramp = (elapsed_s / (p.ramp_days * 86400.0)).clamp(0.0, 1.0);
            let one_sided = v.stance_votes >= p.one_sided_min_votes
                && f64::from(v.agrees.max(v.stance_votes - v.agrees)) / f64::from(v.stance_votes)
                    > p.one_sided_share;
            let weight = if is_excluded {
                0.0
            } else {
                ramp * if one_sided { p.one_sided_factor } else { 1.0 }
            };
            let mut bits = 0;
            if is_excluded {
                bits |= REASON_EXCLUDED;
            } else {
                if ramp < 1.0 {
                    bits |= REASON_NEW_ACCOUNT;
                }
                if one_sided {
                    bits |= REASON_ONE_SIDED;
                }
            }
            AccountWeight {
                user_id: v.user_id,
                weight,
                vote_count: v.vote_count,
                stance_vote_count: v.stance_votes,
                first_vote_us: Some(v.first_vote_us),
                computed_us: as_of_us,
                reason_bits: bits as i32,
            }
        })
        .collect())
}

/// How many of `rust` differ from `table` (by user, on every mirrored field), counting a row
/// present on one side only. The shadow check a run logs before the SQL is retired.
pub fn count_mismatches(rust: &[AccountWeight], table: &[AccountWeight]) -> usize {
    let by_user: std::collections::HashMap<Uuid, &AccountWeight> =
        table.iter().map(|r| (r.user_id, r)).collect();
    let differing = rust
        .iter()
        .filter(|r| by_user.get(&r.user_id).is_none_or(|t| *t != *r))
        .count();
    let rust_users: HashSet<Uuid> = rust.iter().map(|r| r.user_id).collect();
    differing
        + table
            .iter()
            .filter(|t| !rust_users.contains(&t.user_id))
            .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voter, whether excluded, then the SQL's weight, reason bits and computed_at.
    type GoldenRow = (VoterAgg, bool, f64, u32, i64);

    const GOLDEN: &str = include_str!("../tests/fixtures/account_weights_golden.txt");

    /// The golden sections: each `P` line starts one parameter set, its `U` lines follow.
    fn sections() -> Vec<(Params, Vec<GoldenRow>)> {
        let mut out: Vec<(Params, Vec<GoldenRow>)> = Vec::new();
        for line in GOLDEN.lines() {
            let f: Vec<&str> = line.split('|').collect();
            match f[0] {
                "P" => out.push((
                    Params {
                        ramp_days: f[1].parse().unwrap(),
                        one_sided_min_votes: f[2].parse().unwrap(),
                        one_sided_share: f[3].parse().unwrap(),
                        one_sided_factor: f[4].parse().unwrap(),
                    },
                    Vec::new(),
                )),
                "U" => out.last_mut().unwrap().1.push((
                    VoterAgg {
                        user_id: f[1].parse().unwrap(),
                        vote_count: f[2].parse().unwrap(),
                        stance_votes: f[3].parse().unwrap(),
                        agrees: f[4].parse().unwrap(),
                        first_vote_us: f[5].parse().unwrap(),
                    },
                    f[6] == "1",
                    f[7].parse().unwrap(),
                    f[8].parse().unwrap(),
                    f[9].parse().unwrap(),
                )),
                _ => {}
            }
        }
        out
    }

    /// 2026-11-06 12:00:00+00, the `as_of` the grid ran at.
    const AS_OF_US: i64 = 1_793_966_400_000_000;

    #[test]
    fn matches_the_sql_over_the_grid() {
        let sections = sections();
        assert_eq!(sections.len(), 2, "the golden grid is missing");
        for (params, rows) in sections {
            assert!(rows.len() >= 300, "the grid is too small: {}", rows.len());
            let voters: Vec<VoterAgg> = rows.iter().map(|r| r.0.clone()).collect();
            let excluded: HashSet<Uuid> =
                rows.iter().filter(|r| r.1).map(|r| r.0.user_id).collect();
            let got = compute(&voters, &excluded, AS_OF_US, &params).unwrap();
            assert_eq!(got.len(), rows.len(), "{params:?}: one row per voter");
            for (g, (v, _, weight, bits, computed)) in got.iter().zip(&rows) {
                assert_eq!(
                    g.weight.to_bits(),
                    weight.to_bits(),
                    "{params:?} {v:?}: weight"
                );
                assert_eq!(g.reason_bits as u32, *bits, "{params:?} {v:?}: reasons");
                assert_eq!(g.computed_us, *computed);
                assert_eq!(g.vote_count, v.vote_count);
                assert_eq!(g.stance_vote_count, v.stance_votes);
                assert_eq!(g.first_vote_us, Some(v.first_vote_us));
            }
        }
    }

    #[test]
    fn the_grid_exercises_every_reason() {
        let mut seen = 0u32;
        for (_, rows) in sections() {
            for r in rows {
                seen |= r.3;
            }
        }
        assert_eq!(seen, 7);
    }

    #[test]
    fn invalid_parameters_are_refused_like_the_sql() {
        let ok = Params::default();
        assert!(ok.validate().is_ok());
        for bad in [
            Params {
                ramp_days: 0.0,
                ..ok
            },
            Params {
                one_sided_min_votes: 0,
                ..ok
            },
            Params {
                one_sided_share: 0.5,
                ..ok
            },
            Params {
                one_sided_share: 1.01,
                ..ok
            },
            Params {
                one_sided_factor: -0.1,
                ..ok
            },
            Params {
                one_sided_factor: 1.1,
                ..ok
            },
        ] {
            assert!(compute(&[], &HashSet::new(), 0, &bad).is_err(), "{bad:?}");
        }
        assert!(Params {
            one_sided_share: 1.0,
            ..ok
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn a_row_the_table_lacks_or_has_extra_is_a_mismatch() {
        let a = |n: u8| AccountWeight {
            user_id: Uuid::from_bytes([n; 16]),
            weight: 1.0,
            vote_count: 1,
            stance_vote_count: 0,
            first_vote_us: Some(1),
            computed_us: 2,
            reason_bits: 0,
        };
        assert_eq!(count_mismatches(&[a(1), a(2)], &[a(1), a(2)]), 0);
        assert_eq!(count_mismatches(&[a(1), a(2)], &[a(1)]), 1);
        assert_eq!(count_mismatches(&[a(1)], &[a(1), a(3)]), 1);
        let mut b = a(1);
        b.weight = 0.5;
        assert_eq!(count_mismatches(&[a(1)], &[b]), 1);
    }
}
