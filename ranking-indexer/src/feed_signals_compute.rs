//! Rust twin of the pure part of `personalization.feed_signal_window` and
//! `personalization.replace_feed_signals` (migrations 0109, 0112; GEO-3235): which window a run
//! replaces, and what the replacement writes and refuses. Times are epoch seconds (UTC), days are
//! epoch days. The SQL stays the source until cutover; `feed_signals_sync` runs this as a shadow
//! behind FEED_SIGNALS_RUST_COMPUTE and only compares.
//!
//! Not modelled: the `feed_signal_runs` log, and a duplicate (item, hour) in one input (a primary
//! key violation in SQL; analytics returns one row per pair).

use std::collections::{BTreeMap, BTreeSet};

use crate::wallet_hash::wallet_address_hash;

const DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub revise_days: i64,
    pub backfill_days: i64,
    pub user_retention_days: i64,
    pub item_retention_days: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub last_data_through: Option<i64>,
    pub unchanged: bool,
}

/// `feed_signal_window(p_data_through)` given the last successful run's watermark.
pub fn window(cfg: &Config, data_through: i64, last: Option<i64>) -> Window {
    let revise_from = data_through - cfg.revise_days * DAY;
    // No successful run is `-infinity`: the first run backfills the whole `backfill_days`.
    let from_last = last.unwrap_or(i64::MIN).min(revise_from);
    let start = from_last
        .max(data_through - cfg.backfill_days * DAY)
        .div_euclid(DAY)
        * DAY;
    Window {
        start,
        end: data_through,
        last_data_through: last,
        unchanged: last.is_some_and(|l| data_through <= l),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_id: Option<String>,
    pub hour: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserRow {
    pub wallet_address_hash: String,
    pub item_id: Option<String>,
    pub day: i64,
    pub impressions: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Space {
    pub id: String,
    pub kind: String,
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Seen {
    pub user_id: String,
    pub day: i64,
    pub item_id: String,
    pub impressions: i64,
    pub last_seen_at: i64,
}

/// What the tables hold before the run: item keys and per-user rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub items: BTreeSet<(String, i64)>,
    pub seen: BTreeSet<Seen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    EmptyWindow,
    NotMidnight,
    NoItems,
    NoPlacement,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub item_rows: i64,
    pub user_rows_in: i64,
    pub user_rows_placed: i64,
    pub user_rows: i64,
    pub item_rows_expired: i64,
    pub user_rows_expired: i64,
    pub state: State,
}

#[allow(clippy::too_many_arguments)]
pub fn replace(
    cfg: &Config,
    state: &State,
    window_start: i64,
    window_end: i64,
    items: &[Item],
    users: &[UserRow],
    spaces: &[Space],
    allow_empty: bool,
    as_of: i64,
) -> Result<Outcome, Refusal> {
    if window_end <= window_start {
        return Err(Refusal::EmptyWindow);
    }
    if window_start.rem_euclid(DAY) != 0 {
        return Err(Refusal::NotMidnight);
    }
    let start_day = window_start.div_euclid(DAY);

    if items.is_empty() && !allow_empty && state.items.iter().any(|(_, h)| *h >= window_start) {
        return Err(Refusal::NoItems);
    }
    let mut out = state.clone();
    out.items.retain(|(_, h)| *h < window_start);
    let mut item_rows = 0;
    for i in items {
        if let Some(id) = &i.item_id {
            if i.hour >= window_start && i.hour < window_end {
                out.items.insert((id.clone(), i.hour));
                item_rows += 1;
            }
        }
    }

    // Each user row lands on every personal space whose address hashes to it.
    let hashes: Vec<(&Space, String)> = spaces
        .iter()
        .filter(|s| s.kind == "Personal")
        .map(|s| (s, wallet_address_hash(&s.address)))
        .collect();
    let mut placed_rows: Vec<(&str, &UserRow)> = Vec::new();
    for u in users {
        if u.item_id.is_none() || u.day < start_day || u.last_seen_at >= window_end {
            continue;
        }
        let want = u.wallet_address_hash.trim_matches(' ').to_lowercase();
        for (s, h) in &hashes {
            if *h == want {
                placed_rows.push((s.id.as_str(), u));
            }
        }
    }
    let user_rows_in = users.len() as i64;
    let user_rows_placed = placed_rows.len() as i64;
    if user_rows_in > 0
        && user_rows_placed == 0
        && !allow_empty
        && state.seen.iter().any(|s| s.day >= start_day)
    {
        return Err(Refusal::NoPlacement);
    }

    out.seen.retain(|s| s.day < start_day);
    // One row per (user, day, item): impressions summed, last seen the latest.
    let mut grouped: BTreeMap<(String, i64, String), (i64, i64)> = BTreeMap::new();
    for (user, u) in placed_rows {
        let key = (
            user.to_string(),
            u.day,
            u.item_id.clone().unwrap_or_default(),
        );
        let e = grouped.entry(key).or_insert((0, i64::MIN));
        e.0 += u.impressions;
        e.1 = e.1.max(u.last_seen_at);
    }
    let user_rows = grouped.len() as i64;
    for ((user_id, day, item_id), (impressions, last_seen_at)) in grouped {
        out.seen.insert(Seen {
            user_id,
            day,
            item_id,
            impressions,
            last_seen_at,
        });
    }

    // Retention, every run, after the insert.
    let user_cut = as_of.div_euclid(DAY) - cfg.user_retention_days;
    let before = out.seen.len();
    out.seen.retain(|s| s.day >= user_cut);
    let user_rows_expired = (before - out.seen.len()) as i64;
    let item_cut = as_of - cfg.item_retention_days * DAY;
    let before = out.items.len();
    out.items.retain(|(_, h)| *h >= item_cut);
    let item_rows_expired = (before - out.items.len()) as i64;

    Ok(Outcome {
        item_rows,
        user_rows_in,
        user_rows_placed,
        user_rows,
        item_rows_expired,
        user_rows_expired,
        state: out,
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// The two invariants `replace_feed_signals` leans on Postgres for (migration 0112:75 and on; plan 10.21 section 10 row 2).
//
// 1. ONE RUN AT A TIME. The SQL takes `pg_advisory_xact_lock('personalization.feed_signals')`. The CronJob's
//    `concurrencyPolicy: Forbid` does not cover the manual `--dry-run` Job, which does the whole replacement and rolls
//    it back: it reads the same tables and must not interleave with a real run. Once the replacement is frames to geo-db
//    there is no lock that spans calls (each frame is atomic under `derived_lock`, nothing holds across frames), so the
//    writer owes the exclusion itself: [`Lease`]. Every run takes it, a dry run included, and a run that cannot take it
//    does nothing.
//
// 2. THE EMPTY-WINDOW GUARD IS ATOMIC WITH THE REPLACEMENT. `replace` refuses an empty item list when the table holds
//    rows in the window (`Refusal::NoItems`), unless `--allow-empty`. That count and the delete that follows must see one
//    state: a guard that passed on a count another run has since changed would erase a non-empty window. The SQL gets this
//    from the lock and one transaction; here the read carries a version and [`Store::commit`] refuses the write if the
//    version moved (`Contention::Stale`), so the guard's count is the count it replaces.
//
// Both are models of what the port must do, stated and tested here so the geo-db writer has something to match.
// ---------------------------------------------------------------------------------------------------------------------

/// Who holds the single-run lease. A dry run is a holder like any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    CronJob,
    DryRun,
}

/// Why a run did not write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contention {
    /// Another run holds the lease; this run does nothing.
    Held(Holder),
    /// The state moved between the read the guard judged and the write.
    Stale,
}

/// The one-run-at-a-time lease. Not time-based: a holder that dies is the lock service's concern (a Lease object with
/// a TTL, in the port); this fixes only that there is one and who may take it.
#[derive(Debug, Default)]
pub struct Lease {
    held: Option<Holder>,
}

impl Lease {
    pub fn acquire(&mut self, who: Holder) -> Result<(), Contention> {
        match self.held {
            Some(other) => Err(Contention::Held(other)),
            None => {
                self.held = Some(who);
                Ok(())
            }
        }
    }

    pub fn release(&mut self) {
        self.held = None;
    }
}

/// The tables as the writer sees them: a state and a version that moves on every committed write.
#[derive(Debug, Default)]
pub struct Store {
    pub state: State,
    version: u64,
}

impl Store {
    /// The state a run judges its guard on, and the version that guard must still hold at commit.
    pub fn read(&self) -> (State, u64) {
        (self.state.clone(), self.version)
    }

    /// Writes `outcome` only if nothing was committed since `read_at`. A dry run passes `dry_run` and writes nothing.
    pub fn commit(
        &mut self,
        read_at: u64,
        outcome: Outcome,
        dry_run: bool,
    ) -> Result<(), Contention> {
        if self.version != read_at {
            return Err(Contention::Stale);
        }
        if !dry_run {
            self.state = outcome.state;
            self.version += 1;
        }
        Ok(())
    }
}

/// True when FEED_SIGNALS_RUST_COMPUTE is `1`, `true` or `yes`. Off by default.
pub fn shadow_enabled() -> bool {
    matches!(
        std::env::var("FEED_SIGNALS_RUST_COMPUTE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn secs(t: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|t| t.timestamp())
}

fn day_of(d: &str) -> Option<i64> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| {
            d.signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
                .num_days()
        })
}

/// Compares what a run read and the SQL reported with this twin: the window, and the item, user,
/// placed and written counts. The expiry counts depend on the tables before the run and are not
/// compared. Returns one line per disagreement; empty when they agree.
pub fn mismatches(
    cfg: &Config,
    spaces: &[Space],
    data_through: i64,
    sql_window: &crate::feed_signals::Window,
    items: &[crate::feed_signals::ItemRow],
    users: &[crate::feed_signals::UserRow],
    report: &crate::feed_signals::ApplyReport,
) -> Vec<String> {
    let mut out = Vec::new();
    let last = sql_window.last_data_through.map(|t| t.timestamp());
    let w = window(cfg, data_through, last);
    if w.start != sql_window.window_start.timestamp() || w.unchanged != sql_window.unchanged {
        out.push(format!(
            "window: rust start {} unchanged {}, sql start {} unchanged {}",
            w.start,
            w.unchanged,
            sql_window.window_start.timestamp(),
            sql_window.unchanged
        ));
    }
    let (Some(rust_items), Some(rust_users)) = (
        items
            .iter()
            .map(|i| {
                Some(Item {
                    item_id: Some(i.item_id.clone()),
                    hour: secs(&i.hour)?,
                })
            })
            .collect::<Option<Vec<_>>>(),
        users
            .iter()
            .map(|u| {
                Some(UserRow {
                    wallet_address_hash: u.wallet_address_hash.clone(),
                    item_id: Some(u.item_id.clone()),
                    day: day_of(&u.day)?,
                    impressions: i64::from(u.impressions),
                    last_seen_at: secs(&u.last_seen_at)?,
                })
            })
            .collect::<Option<Vec<_>>>(),
    ) else {
        out.push("a time in the rows did not parse".to_string());
        return out;
    };
    // allow_empty: the refusals depend on the tables before the run, which this does not hold.
    match replace(
        cfg,
        &State::default(),
        sql_window.window_start.timestamp(),
        sql_window.window_end.timestamp(),
        &rust_items,
        &rust_users,
        spaces,
        true,
        sql_window.window_end.timestamp(),
    ) {
        Err(r) => out.push(format!("rust refused: {r:?}")),
        Ok(o) => {
            for (name, rust, sql) in [
                ("item_rows", o.item_rows, i64::from(report.item_rows)),
                (
                    "user_rows_in",
                    o.user_rows_in,
                    i64::from(report.user_rows_in),
                ),
                (
                    "user_rows_placed",
                    o.user_rows_placed,
                    i64::from(report.user_rows_placed),
                ),
                ("user_rows", o.user_rows, i64::from(report.user_rows)),
            ] {
                if rust != sql {
                    out.push(format!("{name}: rust {rust}, sql {sql}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const GOLDEN: &str = include_str!("../tests/fixtures/feed_signals_golden.txt");

    fn days(s: &str) -> i64 {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .unwrap()
            .signed_duration_since(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
            .num_days()
    }

    fn opt(s: &str) -> Option<String> {
        (!s.is_empty()).then(|| s.to_string())
    }

    #[derive(Default)]
    struct Case {
        cfg: Option<Config>,
        as_of: i64,
        ws: i64,
        we: i64,
        allow: bool,
        state: State,
        items: Vec<Item>,
        users: Vec<UserRow>,
        expect: Option<Result<[i64; 6], String>>,
        h: BTreeSet<(String, i64)>,
        q: BTreeSet<Seen>,
    }

    #[test]
    fn window_matches_sql() {
        let mut n = 0;
        for l in GOLDEN.lines().filter(|l| l.starts_with("W|")) {
            let f: Vec<&str> = l.split('|').collect();
            let cfg = Config {
                revise_days: f[2].parse().unwrap(),
                backfill_days: f[3].parse().unwrap(),
                user_retention_days: 0,
                item_retention_days: 0,
            };
            let dt: i64 = f[4].parse().unwrap();
            let last: Option<i64> = f[5].parse().ok();
            let w = window(&cfg, dt, last);
            assert_eq!(w.start, f[6].parse::<i64>().unwrap(), "{l}");
            assert_eq!(w.end, f[7].parse::<i64>().unwrap(), "{l}");
            assert_eq!(w.last_data_through, f[8].parse::<i64>().ok(), "{l}");
            assert_eq!(w.unchanged, f[9] == "true", "{l}");
            n += 1;
        }
        assert_eq!(n, 300);
    }

    #[test]
    fn replace_matches_sql() {
        let mut spaces = Vec::new();
        let mut cases: BTreeMap<i64, Case> = BTreeMap::new();
        for l in GOLDEN.lines() {
            let f: Vec<&str> = l.split('|').collect();
            if f[0] == "S" {
                spaces.push(Space {
                    id: f[1].into(),
                    kind: f[2].into(),
                    address: f[3].into(),
                });
                continue;
            }
            if f[0] == "W" {
                continue;
            }
            let n: i64 = f[1].parse().unwrap();
            let c = cases.entry(n).or_default();
            match f[0] {
                "C" => {
                    c.cfg = Some(Config {
                        revise_days: 0,
                        backfill_days: 0,
                        user_retention_days: f[2].parse().unwrap(),
                        item_retention_days: f[3].parse().unwrap(),
                    });
                    c.as_of = f[4].parse().unwrap();
                    c.ws = f[5].parse().unwrap();
                    c.we = f[6].parse().unwrap();
                    c.allow = f[7] == "true";
                }
                "E" => {
                    c.state.items.insert((
                        format!("11111111-0000-0000-0000-{:0>12}", f[2]),
                        f[3].parse().unwrap(),
                    ));
                }
                "F" => {
                    c.state.seen.insert(Seen {
                        user_id: format!("aaaaaaaa-0000-0000-0000-{:0>12}", f[2]),
                        day: days(f[3]),
                        item_id: format!("11111111-0000-0000-0000-{:0>12}", f[4]),
                        impressions: f[5].parse().unwrap(),
                        last_seen_at: f[6].parse().unwrap(),
                    });
                }
                "I" => c.items.push(Item {
                    item_id: opt(f[2]),
                    hour: f[3].parse().unwrap(),
                }),
                "U" => c.users.push(UserRow {
                    wallet_address_hash: f[2].into(),
                    item_id: opt(f[3]),
                    day: days(f[4]),
                    impressions: f[5].parse().unwrap(),
                    last_seen_at: f[6].parse().unwrap(),
                }),
                "Z" if f[2] == "ok" => {
                    let mut v = [0i64; 6];
                    for (i, x) in v.iter_mut().enumerate() {
                        *x = f[3 + i].parse().unwrap();
                    }
                    c.expect = Some(Ok(v));
                }
                "Z" => c.expect = Some(Err(f[3].to_string())),
                "H" => {
                    c.h.insert((f[2].to_string(), f[3].parse().unwrap()));
                }
                "Q" => {
                    c.q.insert(Seen {
                        user_id: f[2].into(),
                        day: days(f[3]),
                        item_id: f[4].into(),
                        impressions: f[5].parse().unwrap(),
                        last_seen_at: f[6].parse().unwrap(),
                    });
                }
                other => panic!("unknown tag {other}"),
            }
        }
        assert_eq!(cases.len(), 400);
        let (mut oks, mut errs) = (0, 0);
        for (n, c) in &cases {
            let got = replace(
                c.cfg.as_ref().unwrap(),
                &c.state,
                c.ws,
                c.we,
                &c.items,
                &c.users,
                &spaces,
                c.allow,
                c.as_of,
            );
            match (c.expect.as_ref().unwrap(), got) {
                (Ok(v), Ok(o)) => {
                    oks += 1;
                    let g = [
                        o.item_rows,
                        o.user_rows_in,
                        o.user_rows_placed,
                        o.user_rows,
                        o.item_rows_expired,
                        o.user_rows_expired,
                    ];
                    assert_eq!(&g, v, "case {n} counts");
                    assert_eq!(o.state.items, c.h, "case {n} items");
                    assert_eq!(o.state.seen, c.q, "case {n} seen");
                }
                (Err(want), Err(r)) => {
                    errs += 1;
                    let class = match r {
                        Refusal::EmptyWindow => "window",
                        Refusal::NotMidnight => "midnight",
                        Refusal::NoItems => "items",
                        Refusal::NoPlacement => "users",
                    };
                    assert_eq!(want, class, "case {n}");
                }
                (want, got) => panic!("case {n}: SQL {want:?}, Rust {got:?}"),
            }
        }
        assert!(oks > 200 && errs > 100, "{oks} ok, {errs} refused");
    }

    #[test]
    fn shadow_agrees_on_a_run_and_flags_a_changed_count() {
        use crate::feed_signals as fs;
        use chrono::{TimeZone, Utc};
        let cfg = Config {
            revise_days: 7,
            backfill_days: 30,
            user_retention_days: 30,
            item_retention_days: 90,
        };
        let dt = Utc.with_ymd_and_hms(2026, 6, 10, 12, 0, 0).unwrap();
        let sql_window = fs::Window {
            window_start: Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap(),
            window_end: dt,
            last_data_through: Some(Utc.with_ymd_and_hms(2026, 6, 10, 11, 0, 0).unwrap()),
            unchanged: false,
        };
        let spaces = [Space {
            id: "s1".into(),
            kind: "Personal".into(),
            address: "0xAbC1".into(),
        }];
        let item = |h: &str| fs::ItemRow {
            item_id: "i1".into(),
            hour: h.into(),
            impressions: 1,
            impressions_top3: 0,
            impressions_4_10: 0,
            impressions_11_30: 0,
            impressions_31_plus: 0,
            opens: 0,
            votes: 0,
            plays: 0,
            explore_plays: 0,
            watch_ms: 0,
            explore_watch_ms: 0,
            plays_with_duration: 0,
            watch_fraction_sum: 0.0,
            completed_plays: 0,
            debate_claim_votes: 0,
        };
        let items = [item("2026-06-09T10:00:00Z"), item("2026-06-02T10:00:00Z")];
        let user = |h: &str| fs::UserRow {
            wallet_address_hash: h.into(),
            item_id: "i1".into(),
            day: "2026-06-09".into(),
            impressions: 2,
            last_seen_at: "2026-06-09T10:00:00Z".into(),
        };
        let users = [user(&wallet_address_hash("0xabc1")), user("sha256:nobody")];
        let report = fs::ApplyReport {
            run_id: 1,
            item_rows: 1,
            user_rows_in: 2,
            user_rows_placed: 1,
            user_rows: 1,
            item_rows_expired: 0,
            user_rows_expired: 0,
            debates_rescored: 0,
        };
        let dt_s = dt.timestamp();
        assert_eq!(
            mismatches(&cfg, &spaces, dt_s, &sql_window, &items, &users, &report),
            Vec::<String>::new()
        );
        let bad = fs::ApplyReport {
            user_rows: 2,
            ..report
        };
        assert_eq!(
            mismatches(&cfg, &spaces, dt_s, &sql_window, &items, &users, &bad),
            vec!["user_rows: rust 1, sql 2".to_string()]
        );
    }

    mod exclusion {
        use super::*;

        fn cfg() -> Config {
            Config {
                revise_days: 2,
                backfill_days: 30,
                user_retention_days: 30,
                item_retention_days: 30,
            }
        }

        const DAY0: i64 = 20_000 * DAY;

        fn held_state() -> State {
            let mut s = State::default();
            s.items.insert(("a".into(), DAY0 + 3600));
            s
        }

        fn item(hour: i64) -> Item {
            Item {
                item_id: Some("a".into()),
                hour,
            }
        }

        fn run(state: &State, items: &[Item], allow: bool) -> Result<Outcome, Refusal> {
            replace(
                &cfg(),
                state,
                DAY0,
                DAY0 + 2 * DAY,
                items,
                &[],
                &[],
                allow,
                DAY0 + 2 * DAY,
            )
        }

        #[test]
        fn a_second_run_cannot_take_the_lease_and_a_dry_run_is_a_run() {
            let mut lease = Lease::default();
            lease.acquire(Holder::CronJob).unwrap();
            assert_eq!(
                lease.acquire(Holder::DryRun),
                Err(Contention::Held(Holder::CronJob))
            );
            assert_eq!(
                lease.acquire(Holder::CronJob),
                Err(Contention::Held(Holder::CronJob))
            );
            lease.release();
            lease.acquire(Holder::DryRun).unwrap();
            assert_eq!(
                lease.acquire(Holder::CronJob),
                Err(Contention::Held(Holder::DryRun))
            );
        }

        #[test]
        fn an_empty_item_list_never_erases_a_held_window_without_allow_empty() {
            assert_eq!(
                run(&held_state(), &[], false).unwrap_err(),
                Refusal::NoItems
            );
            // Nothing held in the window: an empty list is fine.
            assert!(run(&State::default(), &[], false).is_ok());
            // The explicit override erases it.
            assert!(run(&held_state(), &[], true)
                .unwrap()
                .state
                .items
                .is_empty());
        }

        #[test]
        fn a_guard_judged_on_a_state_that_moved_cannot_write() {
            let mut store = Store {
                state: held_state(),
                ..Store::default()
            };
            // Run A reads a window that holds a row and replaces it with one row: allowed.
            let (a_state, a_at) = store.read();
            let a = run(&a_state, &[item(DAY0 + 7200)], false).unwrap();
            // Run B read the same version and judged an empty list against the same held row: refused up front.
            let (b_state, b_at) = store.read();
            assert_eq!(run(&b_state, &[], false).unwrap_err(), Refusal::NoItems);
            // A commits. A later run that read BEFORE A, with allow_empty off and an empty window it judged on the old
            // count, would now delete rows it never counted; its commit is refused instead.
            store.commit(a_at, a, false).unwrap();
            let stale = run(&State::default(), &[], false).unwrap();
            assert_eq!(store.commit(b_at, stale, false), Err(Contention::Stale));
            assert_eq!(store.state.items.len(), 1, "the committed window is intact");
        }

        #[test]
        fn a_dry_run_commits_nothing_and_does_not_move_the_version() {
            let mut store = Store {
                state: held_state(),
                ..Store::default()
            };
            let (s, at) = store.read();
            let out = run(&s, &[item(DAY0 + 7200)], false).unwrap();
            store.commit(at, out, true).unwrap();
            assert_eq!(store.state, held_state());
            // The same read can still commit for real: the dry run did not move the version.
            let out = run(&s, &[item(DAY0 + 7200)], false).unwrap();
            store.commit(at, out, false).unwrap();
            assert!(store.state.items.contains(&("a".to_string(), DAY0 + 7200)));
        }
    }
}
