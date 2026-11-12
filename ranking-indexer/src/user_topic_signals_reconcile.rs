//! One-shot reconcile of geo-db collection 28 (`user_topic_signals`) against Postgres.
//!
//! Why: `not_interested_sync`'s recompute used to delete and re-insert a user's rows without
//! mirroring the delete, and the sweep and the refit only delete identities that were in their own
//! prior. A row dropped that way stays in geo-db for good and inflates the user's private
//! `weights`. This reads each user's group back, compares it with what Postgres holds, and plans a
//! delete for every key geo-db has and Postgres does not, and a put for every row that is missing
//! or whose score or value differs. The plan is one pure function ([`plan_user`]) so it is tested
//! without a database; the puts reuse [`user_topic_signals_mirror::plan`], so the layout cannot
//! drift from what the writers send.
//!
//! The scan is by group, so the users read are Postgres's: everyone `user_topic_signals` holds plus
//! every Personal space ([`USERS_SQL`]), which covers a user whose last row Postgres has dropped.
//! A user in geo-db that is in neither is not found. Counts and kinds only are reported, never an id.

use std::collections::{BTreeMap, HashMap};

use crate::external_interest_mirror::kind_tag;
use crate::private_snapshot::{same_score, Held};
use crate::user_topic_signals_mirror::{self as mirror, Signal};
use geodb_cursor::derived::DerivedRow;

/// One page of users, in id order, after `$1`: every user with a row, and every Personal space.
pub const USERS_SQL: &str = "SELECT id FROM (\
     SELECT user_id AS id FROM personalization.user_topic_signals \
     UNION SELECT id FROM public.spaces WHERE type = 'Personal') u \
     WHERE id > $1 ORDER BY id LIMIT $2";

/// The rows of a page of users: [`mirror::SELECT_SQL`] (bound to `$1` as the epoch) narrowed to `$2`.
pub fn page_rows_sql() -> String {
    format!("{} AND user_id = ANY($2)", mirror::SELECT_SQL)
}

/// Which field of a 36-byte value (or the score) differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Drift {
    Score,
    EventCount,
    LastEvent,
    TopSource,
    ComputedAt,
    /// The value has another length, or its bytes differ in a way none of the above names.
    Shape,
}

impl Drift {
    pub fn label(self) -> &'static str {
        match self {
            Drift::Score => "score",
            Drift::EventCount => "event_count",
            Drift::LastEvent => "last_event_at",
            Drift::TopSource => "top_source",
            Drift::ComputedAt => "computed_at",
            Drift::Shape => "shape",
        }
    }
}

/// The fields in which `held` differs from `want`; empty when they agree.
pub fn drift(want: &DerivedRow, held: &Held) -> Vec<Drift> {
    let mut out = Vec::new();
    if !same_score(want.score, held.score) {
        out.push(Drift::Score);
    }
    if want.value.len() != 36 || held.value.len() != 36 {
        if want.value != held.value {
            out.push(Drift::Shape);
        }
        return out;
    }
    for (field, range) in [
        (Drift::EventCount, 0..4),
        (Drift::LastEvent, 4..12),
        (Drift::TopSource, 12..28),
        (Drift::ComputedAt, 28..36),
    ] {
        if want.value[range.clone()] != held.value[range] {
            out.push(field);
        }
    }
    out
}

/// What one user's group needs.
#[derive(Debug, Default, PartialEq)]
pub struct UserPlan {
    pub puts: Vec<DerivedRow>,
    pub deletes: Vec<Vec<u8>>,
    /// The kind byte of each delete, for the report.
    pub delete_tags: Vec<u8>,
    pub matched: u64,
    pub missing: u64,
    pub drifted: Vec<Drift>,
    /// Held keys that are not a 33-byte collection 28 key; left alone.
    pub foreign: u64,
    /// The scan was cut at its page bound, so a missing row was not judged.
    pub partial: bool,
}

/// Plans one user's group. `want` is what Postgres says the group holds, `held` what the scan
/// returned and `more` whether it was cut. A key geo-db holds that Postgres does not is deleted;
/// a wanted key geo-db lacks is put (unless the scan was cut, when it may simply not have been
/// read); a key in both whose score or value differs is put.
pub fn plan_user(want: &[DerivedRow], held: &[Held], more: bool) -> UserPlan {
    let have: HashMap<&[u8], &Held> = held.iter().map(|h| (h.key.as_slice(), h)).collect();
    let wanted: HashMap<&[u8], &DerivedRow> = want.iter().map(|r| (r.key.as_slice(), r)).collect();
    let mut p = UserPlan {
        partial: more,
        ..UserPlan::default()
    };
    for r in want {
        match have.get(r.key.as_slice()) {
            Some(h) => {
                let d = drift(r, h);
                if d.is_empty() {
                    p.matched += 1;
                } else {
                    p.drifted.extend(d);
                    p.puts.push(r.clone());
                }
            }
            None if more => {}
            None => {
                p.missing += 1;
                p.puts.push(r.clone());
            }
        }
    }
    for h in held {
        if wanted.contains_key(h.key.as_slice()) {
            continue;
        }
        match h.key.as_slice() {
            k if k.len() == 33 && k[32] <= 5 => {
                p.deletes.push(h.key.clone());
                p.delete_tags.push(k[32]);
            }
            _ => p.foreign += 1,
        }
    }
    p
}

/// Groups one page of Postgres rows by user into the rows geo-db should hold for each.
pub fn want_by_user(rows: &[Signal]) -> BTreeMap<u128, Vec<DerivedRow>> {
    let mut m: BTreeMap<u128, Vec<DerivedRow>> = BTreeMap::new();
    for r in mirror::plan(rows, &[], &[]).0 {
        m.entry(r.group).or_default().push(r);
    }
    m
}

/// The kind name of a key's tag byte, for the report.
pub fn tag_name(tag: u8) -> &'static str {
    [
        "vote",
        "comment",
        "debate",
        "follow",
        "interested",
        "not_interested",
    ]
    .into_iter()
    .find(|k| kind_tag(k) == Some(tag))
    .unwrap_or("unknown")
}

/// Totals over a run.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Counts {
    pub users: u64,
    pub users_with_changes: u64,
    pub pg_rows: u64,
    pub held_rows: u64,
    pub matched: u64,
    pub missing: u64,
    pub drifted_rows: u64,
    pub stale: u64,
    pub foreign: u64,
    pub partial_groups: u64,
    pub drift_by_field: BTreeMap<&'static str, u64>,
    pub stale_by_kind: BTreeMap<&'static str, u64>,
}

impl Counts {
    /// Folds one user's plan in (`pg` rows wanted, `held` rows scanned).
    pub fn add(&mut self, pg: usize, held: usize, p: &UserPlan) {
        self.users += 1;
        self.pg_rows += pg as u64;
        self.held_rows += held as u64;
        self.matched += p.matched;
        self.missing += p.missing;
        self.stale += p.deletes.len() as u64;
        self.foreign += p.foreign;
        self.partial_groups += u64::from(p.partial);
        let drifted_rows = p.puts.len() as u64 - p.missing;
        self.drifted_rows += drifted_rows;
        for d in &p.drifted {
            *self.drift_by_field.entry(d.label()).or_default() += 1;
        }
        for t in &p.delete_tags {
            *self.stale_by_kind.entry(tag_name(*t)).or_default() += 1;
        }
        if !p.puts.is_empty() || !p.deletes.is_empty() {
            self.users_with_changes += 1;
        }
    }

    /// Rows the plan would change.
    pub fn changes(&self) -> u64 {
        self.missing + self.drifted_rows + self.stale
    }
}

/// True when applying the plan looks like a broken read rather than a repair: it deletes more than
/// `max_fraction` of what geo-db holds, or Postgres returned no rows at all while geo-db has some.
pub fn looks_broken(c: &Counts, max_fraction: f64) -> bool {
    if c.pg_rows == 0 && c.held_rows > 0 {
        return true;
    }
    c.held_rows > 0 && (c.stale as f64) > max_fraction * (c.held_rows as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn sig(user: u8, topic: u8, kind: &str, count: f64) -> Signal {
        Signal {
            user_id: u(user),
            topic_id: u(topic),
            kind: kind.to_string(),
            decayed_count: count,
            event_count: 7,
            last_event_us: Some(5_000_000),
            top_object_id: u(9),
            computed_us: 6_000_000,
        }
    }

    fn held_of(r: &DerivedRow) -> Held {
        Held {
            key: r.key.clone(),
            score: r.score,
            value: r.value.clone(),
        }
    }

    fn rows(sigs: &[Signal]) -> Vec<DerivedRow> {
        want_by_user(sigs).into_values().flatten().collect()
    }

    fn stale_key(user: u8, topic: u8, tag: u8) -> Held {
        let mut k = vec![user; 16];
        k.extend_from_slice(&[topic; 16]);
        k.push(tag);
        Held {
            key: k,
            score: 3.0,
            value: vec![0; 36],
        }
    }

    #[test]
    fn identical_groups_need_nothing() {
        let want = rows(&[sig(1, 2, "vote", 2.0), sig(1, 3, "follow", 1.0)]);
        let held: Vec<Held> = want.iter().map(held_of).collect();
        let p = plan_user(&want, &held, false);
        assert_eq!(p.matched, 2);
        assert!(p.puts.is_empty() && p.deletes.is_empty());
        assert!(p.drifted.is_empty());
    }

    #[test]
    fn a_key_only_geo_db_holds_is_deleted_with_its_kind() {
        let want = rows(&[sig(1, 2, "vote", 2.0)]);
        let mut held: Vec<Held> = want.iter().map(held_of).collect();
        held.push(stale_key(1, 3, kind_tag("not_interested").unwrap()));
        let p = plan_user(&want, &held, false);
        assert_eq!(p.deletes, vec![held[1].key.clone()]);
        assert_eq!(tag_name(p.delete_tags[0]), "not_interested");
        assert_eq!(p.matched, 1);
        assert!(p.puts.is_empty());
    }

    #[test]
    fn a_user_postgres_has_no_rows_for_loses_everything_in_geo_db() {
        let held = vec![stale_key(4, 1, 0), stale_key(4, 2, 3)];
        let p = plan_user(&[], &held, false);
        assert_eq!(p.deletes.len(), 2);
        assert!(p.puts.is_empty());
    }

    #[test]
    fn a_row_geo_db_lacks_is_put() {
        let want = rows(&[sig(1, 2, "vote", 2.0), sig(1, 3, "vote", 1.0)]);
        let held = vec![held_of(&want[0])];
        let p = plan_user(&want, &held, false);
        assert_eq!(p.missing, 1);
        assert_eq!(p.puts, vec![want[1].clone()]);
    }

    #[test]
    fn a_cut_scan_does_not_judge_missing_rows_but_still_deletes_and_fixes_drift() {
        let want = rows(&[sig(1, 2, "vote", 2.0), sig(1, 3, "vote", 1.0)]);
        let mut drifted = held_of(&want[0]);
        drifted.score = 99.0;
        let held = vec![drifted, stale_key(1, 4, 0)];
        let p = plan_user(&want, &held, true);
        assert!(p.partial);
        assert_eq!(p.missing, 0);
        assert_eq!(p.puts, vec![want[0].clone()]);
        assert_eq!(p.deletes.len(), 1);
    }

    #[test]
    fn drift_names_the_field_that_differs() {
        let want = rows(&[sig(1, 2, "vote", 2.0)]);
        let mut h = held_of(&want[0]);
        h.score = 5.0;
        h.value[0] ^= 1;
        h.value[30] ^= 1;
        assert_eq!(
            drift(&want[0], &h),
            vec![Drift::Score, Drift::EventCount, Drift::ComputedAt]
        );
        h.value.pop();
        assert_eq!(drift(&want[0], &h), vec![Drift::Score, Drift::Shape]);
    }

    #[test]
    fn a_score_within_rounding_is_not_drift() {
        let want = rows(&[sig(1, 2, "vote", 2.0)]);
        let mut h = held_of(&want[0]);
        h.score += 1e-12;
        assert!(drift(&want[0], &h).is_empty());
    }

    #[test]
    fn a_key_that_is_not_a_collection_28_key_is_left_alone() {
        let held = vec![
            Held {
                key: vec![1; 20],
                score: 1.0,
                value: vec![],
            },
            stale_key(1, 2, 9),
        ];
        let p = plan_user(&[], &held, false);
        assert!(p.deletes.is_empty());
        assert_eq!(p.foreign, 2);
    }

    #[test]
    fn counts_fold_users_and_report_kinds_and_fields() {
        let want = rows(&[sig(1, 2, "vote", 2.0), sig(1, 3, "vote", 1.0)]);
        let mut h0 = held_of(&want[0]);
        h0.value[1] ^= 1;
        let held = vec![h0, stale_key(1, 5, 5)];
        let p = plan_user(&want, &held, false);
        let mut c = Counts::default();
        c.add(2, 2, &p);
        c.add(0, 0, &UserPlan::default());
        assert_eq!((c.users, c.users_with_changes), (2, 1));
        assert_eq!((c.missing, c.drifted_rows, c.stale), (1, 1, 1));
        assert_eq!(c.changes(), 3);
        assert_eq!(c.drift_by_field["event_count"], 1);
        assert_eq!(c.stale_by_kind["not_interested"], 1);
    }

    #[test]
    fn a_plan_that_deletes_most_of_geo_db_or_reads_an_empty_postgres_looks_broken() {
        let mut c = Counts {
            pg_rows: 100,
            held_rows: 100,
            stale: 5,
            ..Counts::default()
        };
        assert!(!looks_broken(&c, 0.2));
        c.stale = 30;
        assert!(looks_broken(&c, 0.2));
        c.pg_rows = 0;
        c.stale = 0;
        assert!(looks_broken(&c, 0.2));
        assert!(!looks_broken(&Counts::default(), 0.2));
    }

    #[test]
    fn the_page_query_narrows_the_writers_select() {
        assert!(mirror::SELECT_SQL.ends_with("WHERE computed_at >= $1"));
        assert!(page_rows_sql().ends_with("computed_at >= $1 AND user_id = ANY($2)"));
    }
}
