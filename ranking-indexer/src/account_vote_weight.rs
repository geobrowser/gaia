//! Rust twin of `public.account_vote_weight(voter)` (migration 0101): the weight one voter's
//! votes are multiplied by. A voter with no `account_weights` row counts for nothing (0).
//! `account_weights_refresh` checks it against the SQL behind ACCOUNT_VOTE_WEIGHT_RUST_COMPUTE.

use std::collections::BTreeMap;

pub fn vote_weight(weights: &BTreeMap<String, f64>, voter: &str) -> f64 {
    weights.get(voter).copied().unwrap_or(0.0)
}

/// How many `(voter, sql_weight)` pairs this twin answers differently, by bits.
pub fn count_mismatches(weights: &BTreeMap<String, f64>, sql: &[(String, f64)]) -> usize {
    sql.iter()
        .filter(|(v, w)| vote_weight(weights, v).to_bits() != w.to_bits())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/account_vote_weight_golden.txt");

    #[test]
    fn vote_weight_matches_sql() {
        let mut weights = BTreeMap::new();
        let mut asks = Vec::new();
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            let w: f64 = f[2].parse().unwrap();
            match f[0] {
                "W" => {
                    weights.insert(f[1].to_string(), w);
                }
                "V" => asks.push((f[1].to_string(), w)),
                t => panic!("tag {t}"),
            }
        }
        assert_eq!((weights.len(), asks.len()), (30, 40));
        assert_eq!(count_mismatches(&weights, &asks), 0);
        assert!(asks.iter().any(|(_, w)| *w == 0.0) && asks.iter().any(|(_, w)| *w > 0.0));
    }
}
