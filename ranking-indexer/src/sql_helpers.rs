//! The pure SQL helper functions of gaia Postgres, as Rust functions.
//!
//! Each one here takes arguments only, reads no table, and has a twin in an api migration. Once Postgres is gone
//! a caller that scores, dates or orders in code calls these instead of the SQL function, so none of them needs a
//! database. They are pinned to the SQL by `tests/fixtures/sql_helpers_golden.txt`: the rows are what the
//! migrations' own definitions returned on a scratch Postgres 16 for a grid of arguments, and every test below
//! compares the Rust result to a row.
//!
//! | Rust | SQL | migration |
//! | -- | -- | -- |
//! | [`wilson_lower_bound`] | `public.wilson_lower_bound` | 0073 |
//! | [`binary_entropy_bits`] | `public.binary_entropy_bits` | 0103 |
//! | [`time_zone`], [`today`], [`calendar_day`] | `trending.time_zone`, `trending.today` | 0113 |
//! | [`value_date`] | `trending.value_date` | 0113 |
//! | [`position_order`] | `trending.position_order` | 0115 |
//! | [`CLAIM_SOURCES`] | `trending.claim_sources` | 0118 |
//!
//! Nothing calls these yet: they are the library half of the plpgsql port (geo-db plan 10.11 section 4, group D),
//! used by the group A and B functions' ports as each caller moves. Adding the module changes no behaviour.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc, Weekday};

/// `trending.time_zone()`: the calendar agenda dates, "today" and day counts are read in.
pub fn time_zone() -> &'static str {
    "America/New_York"
}

/// `trending.claim_sources()`: the agenda sources whose rows are claims, in the order the SQL lists them.
pub const CLAIM_SOURCES: [&str; 2] = ["block_item", "agenda_claims"];

/// The z the SQL function defaults to (a 95% interval).
pub const WILSON_DEFAULT_Z: f64 = 1.96;

/// `public.wilson_lower_bound(positive, negative, z, prior_positive, prior_negative)`.
///
/// Lower bound of the Wilson score interval over the observed votes plus a prior, in `[0, 1]`. A missing count
/// is 0 and a negative one is clamped to 0, so `(-5, -5)` is the prior alone. No observations and no prior is
/// 0, not a division by zero. The SQL defaults are `z = 1.96` and a prior of `(1, 1)`; pass them explicitly.
/// Derivation: the Wilson formula, with `phat` the prior-padded positive share and `n` the padded total.
pub fn wilson_lower_bound(
    positive: Option<f64>,
    negative: Option<f64>,
    z: Option<f64>,
    prior_positive: Option<f64>,
    prior_negative: Option<f64>,
) -> f64 {
    let clamp = |v: Option<f64>| v.unwrap_or(0.0).max(0.0);
    let p_obs = clamp(positive) + clamp(prior_positive);
    let n_obs = clamp(negative) + clamp(prior_negative);
    let total = p_obs + n_obs;
    if total <= 0.0 {
        return 0.0;
    }
    let zf = z.unwrap_or(WILSON_DEFAULT_Z);
    let phat = p_obs / total;
    let lower = (phat + (zf * zf) / (2.0 * total)
        - zf * ((phat * (1.0 - phat) + (zf * zf) / (4.0 * total)) / total).sqrt())
        / (1.0 + (zf * zf) / total);
    lower.clamp(0.0, 1.0)
}

/// `public.binary_entropy_bits(p)`: 0 at `p <= 0` or `p >= 1`, 1 at one half, `None` for a missing `p`.
pub fn binary_entropy_bits(p: Option<f64>) -> Option<f64> {
    let p = p?;
    if p <= 0.0 || p >= 1.0 {
        return Some(0.0);
    }
    Some(-(p * p.ln() + (1.0 - p) * (1.0 - p).ln()) / std::f64::consts::LN_2)
}

/// The `n`th (1-based) `weekday` of a month.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: i64) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).expect("a month has a first day");
    let ahead = (weekday.num_days_from_monday() as i64
        - first.weekday().num_days_from_monday() as i64)
        .rem_euclid(7);
    first + Duration::days(ahead + 7 * (n - 1))
}

/// Seconds `America/New_York` is behind UTC at an instant: 18000 (EST) or 14400 (EDT).
///
/// The US rule in force since 2007: daylight time runs from 02:00 standard on the second Sunday of March (07:00
/// UTC) to 02:00 daylight on the first Sunday of November (06:00 UTC). Valid for 2007 onward, which is every
/// agenda there is; an earlier instant gets the same rule, not the old one.
fn new_york_offset_secs(at: DateTime<Utc>) -> i64 {
    let y = at.year();
    let starts = Utc.from_utc_datetime(
        &nth_weekday(y, 3, Weekday::Sun, 2)
            .and_hms_opt(7, 0, 0)
            .unwrap(),
    );
    let ends = Utc.from_utc_datetime(
        &nth_weekday(y, 11, Weekday::Sun, 1)
            .and_hms_opt(6, 0, 0)
            .unwrap(),
    );
    if at >= starts && at < ends {
        4 * 3600
    } else {
        5 * 3600
    }
}

/// `(at AT TIME ZONE trending.time_zone())::date`: the New York calendar day of an instant.
pub fn calendar_day(at: DateTime<Utc>) -> NaiveDate {
    (at - Duration::seconds(new_york_offset_secs(at))).date_naive()
}

/// `trending.today()` at the instant `now` (the SQL reads `now()`; a caller passes its own clock).
pub fn today(now: DateTime<Utc>) -> NaiveDate {
    calendar_day(now)
}

/// `^\d{4}-\d{2}-\d{2}` followed by anything: the date at the head of a string, or `None`.
///
/// `None` when the head is not that shape, and also when it is but is no calendar date (`2026-02-30`, month 13,
/// year 0000), which is where the SQL raises and its exception block turns the whole call into NULL.
enum Head {
    NotShaped,
    Date(Option<NaiveDate>),
}

fn date_head(text: &str) -> Head {
    let b = text.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if b.len() < 10
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
    {
        return Head::NotShaped;
    }
    let num = |r: std::ops::Range<usize>| text[r].parse::<u32>().unwrap_or(0);
    let year = num(0..4) as i32;
    if year == 0 {
        return Head::Date(None); // Postgres has no year 0
    }
    Head::Date(NaiveDate::from_ymd_opt(year, num(5..7), num(8..10)))
}

/// `trending.value_date(datetime_utc, datetime, date)`: a Date value's calendar day.
///
/// The date the writer wrote wins: the head of the `datetime` text, else the head of the `date` text, whatever
/// offset follows (midnight New York and midnight UTC are both that day). With neither, `datetime_utc` read in
/// New York. A head that matches the shape but is no real date is `None` and does not fall through to the next
/// source, which is what the SQL does when its cast raises.
pub fn value_date(
    datetime_utc: Option<DateTime<Utc>>,
    datetime: Option<&str>,
    date: Option<&str>,
) -> Option<NaiveDate> {
    for text in [datetime, date].into_iter().flatten() {
        if let Head::Date(d) = date_head(text) {
            return d;
        }
    }
    datetime_utc.map(calendar_day)
}

/// `trending.position_order(pos)`: the bytes that sort relation positions the way Geo shows them.
///
/// `lower(pos)`, a space, then `pos` with its letter case swapped, as UTF-8 bytes: a case-insensitive compare
/// first, lowercase before uppercase on a tie, the same under any collation. `None` for a missing position
/// (callers sort those last). Letters beyond ASCII follow Rust's Unicode case mapping, not the database
/// locale's; fractional-index positions are `0-9A-Za-z`, so this does not arise.
pub fn position_order(pos: Option<&str>) -> Option<Vec<u8>> {
    let pos = pos?;
    let swapped: String = pos
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect();
    let mut key = pos.to_lowercase().into_bytes();
    key.push(b' ');
    key.extend_from_slice(swapped.as_bytes());
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = include_str!("../tests/fixtures/sql_helpers_golden.txt");

    /// The rows of one kind, split on `|`.
    fn rows(kind: &str) -> Vec<Vec<&'static str>> {
        GOLDEN
            .lines()
            .filter(|l| l.starts_with(kind) && l[kind.len()..].starts_with('|'))
            .map(|l| l.split('|').skip(1).collect())
            .collect()
    }

    fn num(s: &str) -> Option<f64> {
        (!s.is_empty() && s != "N").then(|| s.parse().expect("a number"))
    }

    fn text(s: &str) -> Option<&str> {
        (s != "N").then_some(s)
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_str(&format!("{s}00"), "%Y-%m-%d %H:%M:%S%#z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn wilson_matches_the_sql_over_the_grid() {
        let grid = rows("W");
        assert!(
            grid.len() >= 60,
            "the golden grid is missing: {}",
            grid.len()
        );
        for r in grid {
            let got = wilson_lower_bound(num(r[0]), num(r[1]), num(r[2]), num(r[3]), num(r[4]));
            let want: f64 = r[5].parse().unwrap();
            assert!(
                (got - want).abs() < 1e-12,
                "wilson{r:?}: got {got}, SQL {want}"
            );
        }
    }

    #[test]
    fn wilson_keeps_the_sql_suite_properties() {
        let w = |p, n| wilson_lower_bound(Some(p), Some(n), None, Some(1.0), Some(1.0));
        assert!(w(0.0, 0.0) > 0.0 && w(0.0, 0.0) < 1.0);
        assert!(w(10.0, 0.0) > w(1.0, 0.0));
        assert!(w(0.0, 10.0) < w(0.0, 0.0));
        assert!(w(1.0, 0.0) < w(50.0, 2.0));
        assert!(w(100.0, 90.0) < w(10.0, 0.0));
        assert_eq!(
            wilson_lower_bound(Some(0.0), Some(0.0), Some(1.96), Some(0.0), Some(0.0)),
            0.0
        );
    }

    #[test]
    fn entropy_matches_the_sql() {
        let grid = rows("E");
        assert_eq!(grid.len(), 10);
        for r in grid {
            let got = binary_entropy_bits(num(r[0]));
            match num(r[1]) {
                None => assert_eq!(got, None, "entropy({})", r[0]),
                Some(want) => {
                    let got = got.expect("a value");
                    assert!(
                        (got - want).abs() < 1e-12,
                        "entropy({}): got {got}, SQL {want}",
                        r[0]
                    );
                }
            }
        }
    }

    #[test]
    fn value_date_matches_the_sql_over_the_grid() {
        let grid = rows("D");
        assert!(
            grid.len() >= 150,
            "the golden grid is missing: {}",
            grid.len()
        );
        for r in grid {
            let at = text(r[0]).map(utc);
            let got = value_date(at, text(r[1]), text(r[2])).map(|d| d.to_string());
            assert_eq!(got.as_deref(), text(r[3]), "value_date({r:?})");
        }
    }

    #[test]
    fn new_york_day_matches_the_sql_across_the_dst_edges() {
        let grid = rows("T");
        assert!(
            grid.len() >= 1500,
            "the golden grid is missing: {}",
            grid.len()
        );
        for r in grid {
            assert_eq!(
                calendar_day(utc(r[0])).to_string(),
                r[1],
                "calendar_day({})",
                r[0]
            );
            assert_eq!(today(utc(r[0])).to_string(), r[1]);
        }
        assert_eq!(time_zone(), "America/New_York");
    }

    #[test]
    fn position_order_matches_the_sql_bytes() {
        let grid = rows("P");
        assert_eq!(grid.len(), 10);
        for r in grid {
            let got = position_order(text(r[0]))
                .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>());
            assert_eq!(got.as_deref(), text(r[1]), "position_order({})", r[0]);
        }
    }

    #[test]
    fn position_order_sorts_like_geo_shows_relations() {
        let mut keys = ["b", "A", "a", "B", "aA", "aa"].map(|p| (position_order(Some(p)), p));
        keys.sort();
        let sorted: Vec<&str> = keys.iter().map(|k| k.1).collect();
        assert_eq!(sorted, ["a", "A", "aa", "aA", "b", "B"]);
    }

    #[test]
    fn claim_sources_are_the_sql_array() {
        assert_eq!(CLAIM_SOURCES, ["block_item", "agenda_claims"]);
    }
}
