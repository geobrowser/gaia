//! Mirrors `personalization.user_topic_signals` into geo-db's derived class (plan 10.14 V12,
//! collection 28).
//!
//! Collection 28 `user_topic_signals`: group the user's id as a big-endian `u128`, so one scan is
//! everything learned about a person; key 33 bytes: the user's uuid, the topic's uuid and a kind
//! byte (`external_interest_mirror::kind_tag`); score `decayed_count`, so a scan walks the
//! heaviest topic first; value 36 bytes: `event_count` as a little-endian `i32`, `last_event_at`
//! in microseconds as a little-endian `i64` (`i64::MIN` when null), the top source's uuid, and
//! `computed_at` in microseconds as a little-endian `i64`.
//!
//! Three writers change the table: the two-minute sweep, the nightly refit, and the recompute
//! `not_interested_sync` runs. All of them stamp `computed_at`, so after a sweep the mirror puts
//! the rows with `computed_at` at or after [`SWEEP_LOOKBACK`] ago (longer than the sweep's
//! period, so a recompute between two sweeps is caught by the next), and after the refit it puts
//! the whole table. A delete goes for each identity the table held before the run and no longer
//! has. Last write wins, so a row sent twice is harmless.
//!
//! These rows are one person's private profile, and geo-db does not yet restrict who may read a
//! derived collection (V5). So the mirror needs both `GEODB_DERIVED_MIRROR` and
//! `GEODB_DERIVED_MIRROR_PRIVATE` (see [`from_env_private`]). Enable the second only once geo-db's
//! restricted-read grant exists. It never fails the job (see `geodb_cursor::derived`).

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

use crate::external_interest_mirror::{kind_tag, private_enabled};

/// `user_topic_signals` in geo-db's collection registry.
pub const COLLECTION: u32 = 28;

/// How far back a sweep's mirror looks for rows `computed_at` touched.
pub const SWEEP_LOOKBACK_MINUTES: i64 = 10;

/// The SQL that reads the rows computed at or after `$1`, in the shape [`Signal`] takes.
pub const SELECT_SQL: &str = "SELECT user_id, topic_id, kind, decayed_count, event_count, \
    (extract(epoch FROM last_event_at)::numeric * 1000000)::bigint AS last_event_us, \
    top_object_id, \
    (extract(epoch FROM computed_at)::numeric * 1000000)::bigint AS computed_us \
    FROM personalization.user_topic_signals WHERE computed_at >= $1";

/// The SQL that reads the identities the table holds.
pub const IDENT_SQL: &str =
    "SELECT user_id, topic_id, kind FROM personalization.user_topic_signals";

/// One row of `personalization.user_topic_signals`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Signal {
    pub user_id: Uuid,
    pub topic_id: Uuid,
    pub kind: String,
    pub decayed_count: f64,
    pub event_count: i32,
    pub last_event_us: Option<i64>,
    pub top_object_id: Uuid,
    pub computed_us: i64,
}

/// The identity of a row: what the key is made of.
#[derive(Debug, Clone, PartialEq, Eq, Hash, sqlx::FromRow)]
pub struct Ident {
    pub user_id: Uuid,
    pub topic_id: Uuid,
    pub kind: String,
}

/// The mirror, only when both the shared and the private switch are on.
pub fn from_env_private() -> Option<DerivedMirror> {
    if private_enabled() {
        DerivedMirror::from_env()
    } else {
        None
    }
}

/// The `computed_at` bound a sweep's mirror reads from.
pub fn sweep_since(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::minutes(SWEEP_LOOKBACK_MINUTES)
}

/// The `computed_at` bound the refit's mirror reads from: everything.
pub fn refit_since() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("the epoch is a valid time")
}

fn key(user: &Uuid, topic: &Uuid, tag: u8) -> Vec<u8> {
    let mut k = Vec::with_capacity(33);
    k.extend_from_slice(user.as_bytes());
    k.extend_from_slice(topic.as_bytes());
    k.push(tag);
    k
}

/// The rows to put (`rows`, already limited to the changed ones) and the keys to delete, given
/// the identities before the run (`prior`) and after it (`after`). A kind with no byte is left out.
pub fn plan(rows: &[Signal], prior: &[Ident], after: &[Ident]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .filter_map(|r| {
            let tag = kind_tag(&r.kind)?;
            let mut value = Vec::with_capacity(36);
            value.extend_from_slice(&r.event_count.to_le_bytes());
            value.extend_from_slice(&r.last_event_us.unwrap_or(i64::MIN).to_le_bytes());
            value.extend_from_slice(r.top_object_id.as_bytes());
            value.extend_from_slice(&r.computed_us.to_le_bytes());
            Some(DerivedRow {
                collection: COLLECTION,
                group: r.user_id.as_u128(),
                score: if r.decayed_count.is_finite() {
                    r.decayed_count
                } else {
                    0.0
                },
                key: key(&r.user_id, &r.topic_id, tag),
                value,
            })
        })
        .collect();
    let now: HashSet<(Uuid, Uuid, &str)> = after
        .iter()
        .map(|r| (r.user_id, r.topic_id, r.kind.as_str()))
        .collect();
    let deletes = prior
        .iter()
        .filter(|p| !now.contains(&(p.user_id, p.topic_id, p.kind.as_str())))
        .filter_map(|p| Some(key(&p.user_id, &p.topic_id, kind_tag(&p.kind)?)))
        .collect();
    (puts, deletes)
}

/// Sends the plan for one run. Never fails.
pub async fn mirror(m: &DerivedMirror, rows: &[Signal], prior: &[Ident], after: &[Ident]) {
    let (puts, deletes) = plan(rows, prior, after);
    if !puts.is_empty() {
        m.put("user_topic_signals", &puts).await;
    }
    if !deletes.is_empty() {
        m.delete("user_topic_signals", COLLECTION, &deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn id(user: u8, topic: u8, kind: &str) -> Ident {
        Ident {
            user_id: u(user),
            topic_id: u(topic),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn a_row_is_grouped_by_user_with_a_33_byte_key_and_a_36_byte_value() {
        let (puts, deletes) = plan(&[sig(1, 2, "vote", 2.5)], &[], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 28);
        assert_eq!(p.group, u(1).as_u128());
        assert_eq!(p.score, 2.5);
        let mut want = vec![1u8; 16];
        want.extend_from_slice(&[2u8; 16]);
        want.push(0);
        assert_eq!(p.key, want);
        assert_eq!(p.value.len(), 36);
        assert_eq!(&p.value[0..4], &7i32.to_le_bytes());
        assert_eq!(&p.value[4..12], &5_000_000i64.to_le_bytes());
        assert_eq!(&p.value[12..28], &[9u8; 16]);
        assert_eq!(&p.value[28..36], &6_000_000i64.to_le_bytes());
    }

    #[test]
    fn a_missing_last_event_is_i64_min() {
        let mut s = sig(1, 2, "follow", 1.0);
        s.last_event_us = None;
        let (puts, _) = plan(&[s], &[], &[]);
        assert_eq!(&puts[0].value[4..12], &i64::MIN.to_le_bytes());
    }

    #[test]
    fn two_users_are_two_groups_and_a_non_finite_score_is_zero() {
        let (puts, _) = plan(
            &[sig(1, 3, "vote", f64::NAN), sig(2, 3, "vote", 1.0)],
            &[],
            &[],
        );
        assert_ne!(puts[0].group, puts[1].group);
        assert_eq!(puts[0].score, 0.0);
    }

    #[test]
    fn identities_the_table_lost_are_deleted_and_kept_ones_are_not() {
        let prior = [id(1, 2, "vote"), id(1, 3, "vote"), id(2, 2, "follow")];
        let after = [id(1, 2, "vote")];
        let (puts, deletes) = plan(&[], &prior, &after);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 2);
        assert!(!deletes.contains(&key(&u(1), &u(2), 0)));
        assert!(deletes.contains(&key(&u(1), &u(3), 0)));
        assert!(deletes.contains(&key(&u(2), &u(2), 3)));
    }

    #[test]
    fn a_kind_with_no_byte_is_left_out() {
        let (puts, deletes) = plan(&[sig(1, 2, "mystery", 1.0)], &[id(1, 2, "mystery")], &[]);
        assert!(puts.is_empty());
        assert!(deletes.is_empty());
    }

    #[test]
    fn the_sweep_looks_back_ten_minutes_and_the_refit_from_the_epoch() {
        let now = DateTime::<Utc>::from_timestamp(10_000, 0).unwrap();
        assert_eq!(sweep_since(now).timestamp(), 10_000 - 600);
        assert_eq!(refit_since().timestamp(), 0);
    }
}
