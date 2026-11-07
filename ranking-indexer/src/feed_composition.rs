//! Rust twins of `public.feed_composition_drift` (migration 0093) and of the arithmetic inside
//! `public.sample_feed_composition` (migration 0092), the feed-composition canary's two SQL functions.
//!
//! The canary records what the ranked feed is made of and reports how it moved since the previous sample.
//! Both functions read tables (`entity_feed_composition_samples`, `entity_type_ranking`,
//! `entity_ranking_config`) and call the typed ranked read `entities_ranked_for_feed_by_type`. The ranked
//! window itself is an input here, not recomputed: it is the collection 0 top-N read and has its own port.
//! What is ported is everything after it:
//!
//! * [`drift`]: the per-type count and share moves between the two newest samples of one (scope, source).
//! * [`sample_stats`]: one type label per window row (the earliest of the caller's types the entity has),
//!   the histogram of labels, and the median and oldest age of the rows whose `created_at` is a unix epoch.
//!
//! `tests/fixtures/feed_composition_golden.txt` is the SQL's own output on a scratch postgres:16 with the
//! typed read stubbed to return a given window: 24 windows (empty, one row, two rows, up to 80 rows;
//! `created_at` NULL, empty, not a number, or an epoch; entities with no type, several, or one type twice;
//! window sizes below and above the row count) and 60 drift scenarios (no sample, one, two, three; empty
//! compositions; disjoint keys; two samples in the same instant). Shares are scaled integers (ten
//! thousandths) and compared exactly.
//!
//! Known limit: postgres divides `numeric` to a finite scale before it subtracts, so a share delta whose
//! exact value is a tie at the fifth decimal could round one step differently. [`drift`] works on exact
//! fractions. No case in the grid differs.

use std::collections::{BTreeMap, HashMap};

use uuid::Uuid;

use crate::error::IndexerError;

/// One row of `feed_composition_drift`. Shares and the delta are in ten thousandths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftRow {
    pub type_id: Uuid,
    pub previous_count: i32,
    pub current_count: i32,
    pub previous_share: i64,
    pub current_share: i64,
    pub share_delta: i64,
}

/// `round(num / den, 4)` as ten thousandths, half away from zero. `den` is positive.
fn round4(num: i128, den: i128) -> i64 {
    let scaled = (num.abs() * 20_000 + den) / (2 * den);
    (if num < 0 { -scaled } else { scaled }) as i64
}

fn total(c: &BTreeMap<Uuid, i32>) -> i128 {
    (c.values().map(|v| *v as i128).sum::<i128>()).max(1)
}

/// `feed_composition_drift`: `None` for `previous` is the SQL's empty result (fewer than two samples).
/// Rows are ordered by share delta descending, then type id.
pub fn drift(
    previous: Option<&BTreeMap<Uuid, i32>>,
    current: &BTreeMap<Uuid, i32>,
) -> Vec<DriftRow> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    let (pt, ct) = (total(previous), total(current));
    let mut keys: Vec<Uuid> = previous.keys().chain(current.keys()).copied().collect();
    keys.sort();
    keys.dedup();
    let mut rows: Vec<DriftRow> = keys
        .into_iter()
        .map(|k| {
            let p = previous.get(&k).copied().unwrap_or(0);
            let c = current.get(&k).copied().unwrap_or(0);
            DriftRow {
                type_id: k,
                previous_count: p,
                current_count: c,
                previous_share: round4(p as i128, pt),
                current_share: round4(c as i128, ct),
                share_delta: round4(c as i128 * pt - p as i128 * ct, ct * pt),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.share_delta
            .cmp(&a.share_delta)
            .then(a.type_id.cmp(&b.type_id))
    });
    rows
}

/// One row of the ranked window: its `created_at` text and the type ids `entity_type_ranking` holds for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    pub created_at: Option<String>,
    pub types: Vec<Uuid>,
}

/// What `sample_feed_composition` stores besides the window size and the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleStats {
    pub composition: BTreeMap<Uuid, i64>,
    pub median_age_seconds: Option<i64>,
    pub oldest_age_seconds: Option<i64>,
}

/// The histogram and ages of a window, as of `now_us` (the sample's own timestamp, in microseconds).
/// Only the first `window` rows count (`LIMIT p_window`). Errors when an all-digit `created_at` does not fit
/// a bigint, where the SQL cast would fail the whole sample.
pub fn sample_stats(
    rows: &[WindowRow],
    type_ids: &[Uuid],
    window: usize,
    now_us: i64,
) -> Result<SampleStats, IndexerError> {
    let now_s = (now_us + 500_000).div_euclid(1_000_000);
    let mut composition: BTreeMap<Uuid, i64> = BTreeMap::new();
    let mut ages: Vec<i64> = Vec::new();
    for row in rows.iter().take(window) {
        if let Some(t) = type_ids.iter().find(|t| row.types.contains(t)) {
            *composition.entry(*t).or_default() += 1;
        }
        if let Some(text) = &row.created_at {
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                let at: i64 = text.parse().map_err(|_| {
                    IndexerError::Config("feed composition: created_at out of range".into())
                })?;
                ages.push(now_s - at);
            }
        }
    }
    ages.sort_unstable();
    let median = match ages.len() {
        0 => None,
        n if n % 2 == 1 => Some(ages[n / 2]),
        n => {
            let (a, b) = (ages[n / 2 - 1], ages[n / 2]);
            Some((a as f64 + (b - a) as f64 * 0.5).round_ties_even() as i64)
        }
    };
    Ok(SampleStats {
        composition,
        median_age_seconds: median,
        oldest_age_seconds: ages.last().copied(),
    })
}

/// Entities' type lists by id, from `(entity, type)` rows, duplicates kept.
pub fn types_by_entity(rows: impl IntoIterator<Item = (Uuid, Uuid)>) -> HashMap<Uuid, Vec<Uuid>> {
    let mut m: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (e, t) in rows {
        m.entry(e).or_default().push(t);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/feed_composition_golden.txt");

    fn u(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    #[test]
    fn sample_stats_match_sql() {
        let mut ent: Vec<(Uuid, Uuid)> = Vec::new();
        let mut win: BTreeMap<i32, Vec<(Uuid, Option<String>)>> = BTreeMap::new();
        #[allow(clippy::type_complexity)]
        let mut args: BTreeMap<i32, (Vec<Uuid>, usize, i64, Option<i64>, Option<i64>)> =
            BTreeMap::new();
        let mut hist: BTreeMap<i32, BTreeMap<Uuid, i64>> = BTreeMap::new();
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            let opt = |s: &str| (s != "NULL").then(|| s.parse::<i64>().unwrap());
            match f[0] {
                "E" => ent.push((u(f[1]), u(f[2]))),
                "N" => win
                    .entry(f[1].parse().unwrap())
                    .or_default()
                    .push((u(f[3]), (f[4] != "NULL").then(|| f[4].to_string()))),
                "P" => {
                    args.insert(
                        f[1].parse().unwrap(),
                        (
                            f[2].split(',').map(u).collect(),
                            f[3].parse().unwrap(),
                            f[4].parse().unwrap(),
                            opt(f[5]),
                            opt(f[6]),
                        ),
                    );
                }
                "K" => {
                    hist.entry(f[1].parse().unwrap())
                        .or_default()
                        .insert(u(f[2]), f[3].parse().unwrap());
                }
                _ => {}
            }
        }
        let types = types_by_entity(ent);
        assert_eq!(args.len(), 24);
        for (scn, (tids, w, now_us, median, oldest)) in &args {
            let rows: Vec<WindowRow> = win
                .get(scn)
                .map(|v| v.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|(e, c)| WindowRow {
                    created_at: c.clone(),
                    types: types.get(e).cloned().unwrap_or_default(),
                })
                .collect();
            let got = sample_stats(&rows, tids, *w, *now_us).unwrap();
            assert_eq!(
                &got.composition,
                hist.get(scn).unwrap_or(&BTreeMap::new()),
                "scn {scn}"
            );
            assert_eq!(got.median_age_seconds, *median, "median scn {scn}");
            assert_eq!(got.oldest_age_seconds, *oldest, "oldest scn {scn}");
        }
        // The grid has an empty window, a window cut by the size, and a null median.
        assert!(!win.contains_key(&5) && args.values().any(|a| a.3.is_none()));
        assert!(win.iter().any(|(s, r)| r.len() > args[s].1));
        assert!(args.values().any(|a| a.3.is_some_and(|m| m > 0)));
    }

    #[test]
    fn drift_matches_sql() {
        let mut prev: BTreeMap<i32, BTreeMap<Uuid, i32>> = BTreeMap::new();
        let mut cur: BTreeMap<i32, BTreeMap<Uuid, i32>> = BTreeMap::new();
        let mut samples: BTreeMap<i32, i64> = BTreeMap::new();
        let mut want: BTreeMap<i32, Vec<DriftRow>> = BTreeMap::new();
        let shares = |s: &str| -> i64 {
            let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
            let (i, d) = s.split_once('.').unwrap();
            assert_eq!(d.len(), 4);
            let v = i.parse::<i64>().unwrap() * 10_000 + d.parse::<i64>().unwrap();
            if neg {
                -v
            } else {
                v
            }
        };
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            match f[0] {
                "PC" => {
                    prev.entry(f[1].parse().unwrap())
                        .or_default()
                        .insert(u(f[2]), f[3].parse().unwrap());
                }
                "CC" => {
                    cur.entry(f[1].parse().unwrap())
                        .or_default()
                        .insert(u(f[2]), f[3].parse().unwrap());
                }
                "DS" => {
                    samples.insert(f[1].parse().unwrap(), f[2].parse().unwrap());
                }
                "DR" => want
                    .entry(f[1].parse().unwrap())
                    .or_default()
                    .push(DriftRow {
                        type_id: u(f[2]),
                        previous_count: f[3].parse().unwrap(),
                        current_count: f[4].parse().unwrap(),
                        previous_share: shares(f[5]),
                        current_share: shares(f[6]),
                        share_delta: shares(f[7]),
                    }),
                _ => {}
            }
        }
        // 54 scenarios hold samples; the other 6 hold none and have no rows.
        assert_eq!(samples.len(), 54);
        let empty = BTreeMap::new();
        let mut compared_rows = 0;
        for (scn, n) in &samples {
            let got = if *n >= 2 {
                drift(
                    Some(prev.get(scn).unwrap_or(&empty)),
                    cur.get(scn).unwrap_or(&empty),
                )
            } else {
                drift(None, cur.get(scn).unwrap_or(&empty))
            };
            let mut w = want.remove(scn).unwrap_or_default();
            w.sort_by(|a, b| {
                b.share_delta
                    .cmp(&a.share_delta)
                    .then(a.type_id.cmp(&b.type_id))
            });
            assert_eq!(got, w, "scn {scn}");
            compared_rows += got.len();
        }
        assert!(want.is_empty());
        assert!(compared_rows > 250);
        assert!(samples.values().any(|n| *n == 1) && samples.values().any(|n| *n == 3));
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(round4(1, 32), 313);
        assert_eq!(round4(-1, 32), -313);
        assert_eq!(round4(0, 7), 0);
        assert_eq!(round4(1, 3), 3333);
    }
}
