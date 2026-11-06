//! Mirrors `public.account_exclusions` into geo-db's derived class (plan 10.14, collection 12).
//!
//! Collection 12 `account_exclusions`: group 0, key the excluded account's user id (16 bytes),
//! score 0, value nine bytes: the source tag (`SOURCE_*`) then `synced_at` as a little-endian
//! `i64` of microseconds since the epoch. The `reason` text is not mirrored: `readAccounts`
//! only asks whether a row exists.
//!
//! `replace_account_exclusions()` deletes and re-inserts the analytics rows in one transaction,
//! so the mirror reads the whole table after the commit and sends all of it: every row as a put
//! (last write wins, so unchanged rows are harmless) and a delete for each account the table held
//! before and no longer has. Rows added by hand (source `manual`) are in the table and so are
//! mirrored at the next sync. It never fails the job (see `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `account_exclusions` in geo-db's collection registry.
pub const COLLECTION: u32 = 12;

/// The source tags of the value's first byte.
pub const SOURCE_ANALYTICS: u8 = 0;
pub const SOURCE_MANUAL: u8 = 1;
pub const SOURCE_OTHER: u8 = 2;

/// The SQL that reads the table in the shape [`AccountExclusion`] takes.
pub const SELECT_SQL: &str = "SELECT user_id, \
    (CASE source WHEN 'analytics' THEN 0 WHEN 'manual' THEN 1 ELSE 2 END)::int AS source_tag, \
    (extract(epoch FROM synced_at)::numeric * 1000000)::bigint AS synced_us \
    FROM public.account_exclusions";

/// One row of `public.account_exclusions`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AccountExclusion {
    pub user_id: Uuid,
    pub source_tag: i32,
    pub synced_us: i64,
}

fn value(r: &AccountExclusion) -> Vec<u8> {
    let mut v = Vec::with_capacity(9);
    v.push(r.source_tag as u8);
    v.extend_from_slice(&r.synced_us.to_le_bytes());
    v
}

/// The rows to put and the keys to delete after the sync left `rows`, given the accounts the
/// table held before it.
pub fn plan(rows: &[AccountExclusion], prior: &[Uuid]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: 0,
            score: 0.0,
            key: r.user_id.as_bytes().to_vec(),
            value: value(r),
        })
        .collect();
    let now: std::collections::HashSet<Uuid> = rows.iter().map(|r| r.user_id).collect();
    let deletes = prior
        .iter()
        .filter(|u| !now.contains(u))
        .map(|u| u.as_bytes().to_vec())
        .collect();
    (puts, deletes)
}

/// Sends the plan for one sync. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[AccountExclusion], prior: &[Uuid]) {
    let (puts, deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("account_exclusions", &puts).await;
    }
    if !deletes.is_empty() {
        mirror
            .delete("account_exclusions", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(n: u8, tag: i32) -> AccountExclusion {
        AccountExclusion {
            user_id: u(n),
            source_tag: tag,
            synced_us: 1_700_000_000_000_000,
        }
    }

    #[test]
    fn a_row_is_keyed_by_user_with_a_nine_byte_value() {
        let (puts, deletes) = plan(&[row(1, SOURCE_MANUAL as i32)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 12);
        assert_eq!(p.group, 0);
        assert_eq!(p.score, 0.0);
        assert_eq!(p.key, vec![1u8; 16]);
        assert_eq!(p.value.len(), 9);
        assert_eq!(p.value[0], SOURCE_MANUAL);
        assert_eq!(&p.value[1..9], &1_700_000_000_000_000i64.to_le_bytes());
    }

    #[test]
    fn accounts_the_table_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [row(1, 0)];
        let (puts, deletes) = plan(&rows, &[u(1), u(2), u(3)]);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes, vec![vec![2u8; 16], vec![3u8; 16]]);
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[u(2)]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn the_source_tags_are_distinct() {
        assert_ne!(SOURCE_ANALYTICS, SOURCE_MANUAL);
        assert_ne!(SOURCE_MANUAL, SOURCE_OTHER);
    }
}
