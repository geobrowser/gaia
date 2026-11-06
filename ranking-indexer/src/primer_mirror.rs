//! Mirrors `public.claim_overlap_samples` into geo-db's derived class (plan 10.14, collection 14).
//!
//! Collection 14 `claim_overlap_samples`: group 0, key `sampled_at` as a big-endian `i64` of
//! microseconds since the epoch (8 bytes, so a scan runs oldest first), score the `share` (0.0 when
//! it is null, which happens only when `pairs` is 0), value 32 little-endian bytes: `min_votes`
//! (i32), `min_shared` (i32), `users` (i32), `anchor_set_id` (i32, `-1` when null), `pairs` (i64),
//! `pairs_sharing` (i64).
//!
//! The table gets at most one row a day and is never deleted from, so the mirror reads all of it
//! after the job's Postgres work and puts every row (last write wins, so an unchanged row is
//! harmless). There are no deletes. It never fails the job (see `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};

/// `claim_overlap_samples` in geo-db's collection registry. The one place the number is written.
pub const COLLECTION: u32 = 14;

/// The value's length in bytes.
pub const VALUE_LEN: usize = 32;

/// The SQL that reads the table in the shape [`OverlapSample`] takes.
pub const SELECT_SQL: &str = "SELECT \
    (extract(epoch FROM sampled_at)::numeric * 1000000)::bigint AS sampled_us, \
    min_votes, min_shared, users, pairs, pairs_sharing, share, anchor_set_id \
    FROM public.claim_overlap_samples ORDER BY sampled_at";

/// One row of `public.claim_overlap_samples`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct OverlapSample {
    pub sampled_us: i64,
    pub min_votes: i32,
    pub min_shared: i32,
    pub users: i32,
    pub pairs: i64,
    pub pairs_sharing: i64,
    pub share: Option<f64>,
    pub anchor_set_id: Option<i32>,
}

fn value(r: &OverlapSample) -> Vec<u8> {
    let mut v = Vec::with_capacity(VALUE_LEN);
    v.extend_from_slice(&r.min_votes.to_le_bytes());
    v.extend_from_slice(&r.min_shared.to_le_bytes());
    v.extend_from_slice(&r.users.to_le_bytes());
    v.extend_from_slice(&r.anchor_set_id.unwrap_or(-1).to_le_bytes());
    v.extend_from_slice(&r.pairs.to_le_bytes());
    v.extend_from_slice(&r.pairs_sharing.to_le_bytes());
    v
}

/// The rows to put for the table's current contents.
pub fn plan(rows: &[OverlapSample]) -> Vec<DerivedRow> {
    rows.iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: 0,
            score: r.share.filter(|s| s.is_finite()).unwrap_or(0.0),
            key: r.sampled_us.to_be_bytes().to_vec(),
            value: value(r),
        })
        .collect()
}

/// Sends the plan. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[OverlapSample]) {
    let puts = plan(rows);
    if !puts.is_empty() {
        mirror.put("claim_overlap_samples", &puts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(us: i64, share: Option<f64>, anchor: Option<i32>) -> OverlapSample {
        OverlapSample {
            sampled_us: us,
            min_votes: 5,
            min_shared: 3,
            users: 120,
            pairs: 7140,
            pairs_sharing: 1892,
            share,
            anchor_set_id: anchor,
        }
    }

    #[test]
    fn the_collection_is_14() {
        assert_eq!(COLLECTION, 14);
    }

    #[test]
    fn a_row_is_keyed_by_time_with_a_32_byte_value() {
        let puts = plan(&[sample(1_700_000_000_000_000, Some(0.265), Some(7))]);
        let p = &puts[0];
        assert_eq!(p.collection, COLLECTION);
        assert_eq!(p.group, 0);
        assert_eq!(p.score, 0.265);
        assert_eq!(p.key, 1_700_000_000_000_000i64.to_be_bytes().to_vec());
        assert_eq!(p.value.len(), VALUE_LEN);
        assert_eq!(&p.value[0..4], &5i32.to_le_bytes());
        assert_eq!(&p.value[4..8], &3i32.to_le_bytes());
        assert_eq!(&p.value[8..12], &120i32.to_le_bytes());
        assert_eq!(&p.value[12..16], &7i32.to_le_bytes());
        assert_eq!(&p.value[16..24], &7140i64.to_le_bytes());
        assert_eq!(&p.value[24..32], &1892i64.to_le_bytes());
    }

    #[test]
    fn null_share_scores_zero_and_null_anchor_is_minus_one() {
        let p = &plan(&[sample(1, None, None)])[0];
        assert_eq!(p.score, 0.0);
        assert_eq!(&p.value[12..16], &(-1i32).to_le_bytes());
    }

    #[test]
    fn a_non_finite_share_is_zero_not_sent_as_nan() {
        assert_eq!(plan(&[sample(1, Some(f64::NAN), None)])[0].score, 0.0);
    }

    #[test]
    fn keys_sort_oldest_first_and_an_empty_table_sends_nothing() {
        let puts = plan(&[sample(10, None, None), sample(300, None, None)]);
        assert!(puts[0].key < puts[1].key);
        assert!(plan(&[]).is_empty());
    }
}
