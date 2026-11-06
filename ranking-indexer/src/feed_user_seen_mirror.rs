//! Mirrors `personalization.feed_user_seen_daily` into geo-db's derived class (plan 10.14 V12,
//! collection 29).
//!
//! Collection 29 `feed_user_seen_daily`: group the viewer's user id as a big-endian `u128`, so one
//! scan is everything a person was shown (`feed_user_seen(user, since)` is that scan, summed per
//! item); key 36 bytes: the user's uuid, the UTC day as days since 1970-01-01 in a big-endian
//! `i32`, and the item's uuid; score `last_seen_at` in epoch seconds, so a scan walks the latest
//! first; value 12 bytes: `impressions` as a little-endian `i32` and `last_seen_at` in
//! microseconds as a little-endian `i64`.
//!
//! `replace_feed_signals` (run by `feed_signals_sync`) replaces every row of the window and then
//! deletes the rows past `user_retention_days`, in one transaction. So the mirror reads the
//! identities the table held before the run and the whole table after the commit (about ten
//! thousand rows, far under `derived_rows_max`), sends every row as a put (last write wins, so an
//! unchanged row is harmless) and a delete of each identity that dropped out. That covers both
//! the window replace and the retention prune, since both only remove rows.
//!
//! These rows are one person's private history, and geo-db does not yet restrict who may read a
//! derived collection (V5). So the mirror needs both `GEODB_DERIVED_MIRROR` and
//! `GEODB_DERIVED_MIRROR_PRIVATE` (see [`from_env_private`]). It never fails the job (see
//! `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

use crate::external_interest_mirror::private_enabled;

/// `feed_user_seen_daily` in geo-db's collection registry.
pub const COLLECTION: u32 = 29;

/// The SQL that reads the table in the shape [`Seen`] takes. `day_num` is days since the epoch.
pub const SELECT_SQL: &str = "SELECT user_id, (day - DATE '1970-01-01') AS day_num, item_id, \
    impressions, (extract(epoch FROM last_seen_at)::numeric * 1000000)::bigint AS last_seen_us \
    FROM personalization.feed_user_seen_daily";

/// The SQL that reads the identities the table holds now, before a run changes it.
pub const PRIOR_SQL: &str = "SELECT user_id, (day - DATE '1970-01-01') AS day_num, item_id \
    FROM personalization.feed_user_seen_daily";

/// One row of `personalization.feed_user_seen_daily`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Seen {
    pub user_id: Uuid,
    pub day_num: i32,
    pub item_id: Uuid,
    pub impressions: i32,
    pub last_seen_us: i64,
}

/// A row's identity: user, day (days since the epoch) and item.
pub type Ident = (Uuid, i32, Uuid);

/// The mirror, only when both the shared and the private switch are on.
pub fn from_env_private() -> Option<DerivedMirror> {
    if private_enabled() {
        DerivedMirror::from_env()
    } else {
        None
    }
}

/// A row's geo-db key.
pub fn key(id: &Ident) -> Vec<u8> {
    let mut k = Vec::with_capacity(36);
    k.extend_from_slice(id.0.as_bytes());
    k.extend_from_slice(&id.1.to_be_bytes());
    k.extend_from_slice(id.2.as_bytes());
    k
}

/// The puts, and the keys to delete, after the run left `rows`, given the identities the table
/// held before it.
pub fn plan(rows: &[Seen], prior: &[Ident]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| {
            let mut value = Vec::with_capacity(12);
            value.extend_from_slice(&r.impressions.to_le_bytes());
            value.extend_from_slice(&r.last_seen_us.to_le_bytes());
            DerivedRow {
                collection: COLLECTION,
                group: r.user_id.as_u128(),
                score: (r.last_seen_us as f64) / 1_000_000.0,
                key: key(&(r.user_id, r.day_num, r.item_id)),
                value,
            }
        })
        .collect();
    let now: HashSet<Ident> = rows
        .iter()
        .map(|r| (r.user_id, r.day_num, r.item_id))
        .collect();
    let deletes = prior
        .iter()
        .filter(|id| !now.contains(*id))
        .map(key)
        .collect();
    (puts, deletes)
}

/// Sends the plan for one run. Never fails.
pub async fn mirror(m: &DerivedMirror, rows: &[Seen], prior: &[Ident]) {
    let (puts, mut deletes) = plan(rows, prior);
    if !puts.is_empty() {
        m.put("feed_user_seen_daily", &puts).await;
    }
    if !deletes.is_empty() {
        deletes.sort();
        m.delete("feed_user_seen_daily", COLLECTION, &deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn seen(user: u8, day: i32, item: u8, n: i32, at: i64) -> Seen {
        Seen {
            user_id: u(user),
            day_num: day,
            item_id: u(item),
            impressions: n,
            last_seen_us: at,
        }
    }

    #[test]
    fn a_row_is_grouped_by_user_with_a_36_byte_key_and_a_12_byte_value() {
        let (puts, deletes) = plan(&[seen(1, 20_000, 2, 3, 90_000_000)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 29);
        assert_eq!(p.group, u(1).as_u128());
        assert_eq!(p.score, 90.0);
        let mut want = vec![1u8; 16];
        want.extend_from_slice(&20_000i32.to_be_bytes());
        want.extend_from_slice(&[2u8; 16]);
        assert_eq!(p.key, want);
        assert_eq!(p.value.len(), 12);
        assert_eq!(&p.value[0..4], &3i32.to_le_bytes());
        assert_eq!(&p.value[4..12], &90_000_000i64.to_le_bytes());
    }

    #[test]
    fn two_days_of_one_item_are_two_keys_in_one_group() {
        let (puts, _) = plan(&[seen(1, 1, 2, 1, 1), seen(1, 2, 2, 1, 1)], &[]);
        assert_eq!(puts[0].group, puts[1].group);
        assert_ne!(puts[0].key, puts[1].key);
    }

    #[test]
    fn rows_the_prune_or_replace_removed_are_deleted_and_kept_ones_are_not() {
        let rows = [seen(1, 30, 2, 1, 1)];
        let prior = [(u(1), 30, u(2)), (u(1), 10, u(2)), (u(3), 29, u(4))];
        let (puts, deletes) = plan(&rows, &prior);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes.len(), 2);
        assert!(!deletes.contains(&key(&(u(1), 30, u(2)))));
        assert!(deletes.contains(&key(&(u(1), 10, u(2)))));
        assert!(deletes.contains(&key(&(u(3), 29, u(4)))));
    }

    #[test]
    fn an_empty_table_after_the_run_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[(u(1), 1, u(2))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }
}
