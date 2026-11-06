//! Mirrors `public.account_weights` into geo-db's derived class (plan 10.14, collection 5).
//!
//! Collection 5 `account_weights`: group 0, key the account's user id (16 bytes), score the weight
//! in [0, 1], value 28 little-endian bytes: `vote_count` (i32), `stance_vote_count` (i32),
//! `first_vote_at` (i64 microseconds since the epoch, `i64::MIN` when null), `computed_at` (i64
//! microseconds) and a `u32` of the reason codes (`REASON_*`). The reasons' `detail` text is not
//! mirrored: only `readAccounts` reads the table and it uses `weight` alone.
//!
//! `refresh_account_weights()` rewrites the table in one transaction, so the mirror reads the
//! table after the commit and sends the whole of it: every row as a put (last write wins, so
//! unchanged rows are harmless) and a delete for each account the table held before and no longer
//! has. It never fails the job (see `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `account_weights` in geo-db's collection registry.
pub const COLLECTION: u32 = 5;

/// The reason codes `refresh_account_weights()` writes (migration 0101), one bit each.
pub const REASON_EXCLUDED: u32 = 1;
pub const REASON_NEW_ACCOUNT: u32 = 2;
pub const REASON_ONE_SIDED: u32 = 4;

/// The SQL that reads the table in the shape [`AccountWeight`] takes.
pub const SELECT_SQL: &str = "SELECT user_id, weight, vote_count, stance_vote_count, \
    (extract(epoch FROM first_vote_at)::numeric * 1000000)::bigint AS first_vote_us, \
    (extract(epoch FROM computed_at)::numeric * 1000000)::bigint AS computed_us, \
    ((CASE WHEN reasons @> '[{\"code\": \"excluded\"}]'::jsonb THEN 1 ELSE 0 END) \
     | (CASE WHEN reasons @> '[{\"code\": \"new_account\"}]'::jsonb THEN 2 ELSE 0 END) \
     | (CASE WHEN reasons @> '[{\"code\": \"one_sided\"}]'::jsonb THEN 4 ELSE 0 END))::int AS reason_bits \
    FROM public.account_weights";

/// One row of `public.account_weights`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AccountWeight {
    pub user_id: Uuid,
    pub weight: f64,
    pub vote_count: i32,
    pub stance_vote_count: i32,
    pub first_vote_us: Option<i64>,
    pub computed_us: i64,
    pub reason_bits: i32,
}

fn value(r: &AccountWeight) -> Vec<u8> {
    let mut v = Vec::with_capacity(28);
    v.extend_from_slice(&r.vote_count.to_le_bytes());
    v.extend_from_slice(&r.stance_vote_count.to_le_bytes());
    v.extend_from_slice(&r.first_vote_us.unwrap_or(i64::MIN).to_le_bytes());
    v.extend_from_slice(&r.computed_us.to_le_bytes());
    v.extend_from_slice(&(r.reason_bits as u32).to_le_bytes());
    v
}

/// The rows to put and the keys to delete after the refresh left `rows`, given the accounts the
/// table held before it.
pub fn plan(rows: &[AccountWeight], prior: &[Uuid]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: 0,
            score: r.weight,
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

/// Sends the plan for one refresh. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[AccountWeight], prior: &[Uuid]) {
    let (puts, deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("account_weights", &puts).await;
    }
    if !deletes.is_empty() {
        mirror.delete("account_weights", COLLECTION, &deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(n: u8, weight: f64, first: Option<i64>, bits: i32) -> AccountWeight {
        AccountWeight {
            user_id: u(n),
            weight,
            vote_count: 12,
            stance_vote_count: 5,
            first_vote_us: first,
            computed_us: 1_700_000_000_000_000,
            reason_bits: bits,
        }
    }

    #[test]
    fn a_row_is_keyed_by_user_with_the_weight_as_score() {
        let (puts, deletes) = plan(&[row(1, 0.25, Some(99), REASON_NEW_ACCOUNT as i32)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 5);
        assert_eq!(p.group, 0);
        assert_eq!(p.score, 0.25);
        assert_eq!(p.key, vec![1u8; 16]);
        assert_eq!(p.value.len(), 28);
        assert_eq!(&p.value[0..4], &12i32.to_le_bytes());
        assert_eq!(&p.value[4..8], &5i32.to_le_bytes());
        assert_eq!(&p.value[8..16], &99i64.to_le_bytes());
        assert_eq!(&p.value[16..24], &1_700_000_000_000_000i64.to_le_bytes());
        assert_eq!(&p.value[24..28], &2u32.to_le_bytes());
    }

    #[test]
    fn a_missing_first_vote_is_i64_min() {
        let (puts, _) = plan(&[row(1, 0.0, None, REASON_EXCLUDED as i32)], &[]);
        assert_eq!(&puts[0].value[8..16], &i64::MIN.to_le_bytes());
        assert_eq!(&puts[0].value[24..28], &1u32.to_le_bytes());
    }

    #[test]
    fn accounts_the_table_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [row(1, 1.0, None, 0)];
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
    fn the_reason_bits_are_distinct_and_fit_a_u32() {
        assert_eq!(REASON_EXCLUDED | REASON_NEW_ACCOUNT | REASON_ONE_SIDED, 7);
    }
}
