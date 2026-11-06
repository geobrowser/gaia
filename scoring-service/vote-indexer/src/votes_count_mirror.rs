//! Mirrors `votes_count` into geo-db's derived class (plan 10.14, collection 26).
//!
//! Collection 26 `votes_count`: one row per object, Space and vote kind, the vote tallies the API
//! and the notification indexer read. Group the object id as a big-endian `u128`, because the
//! readers ask for one object's tallies. Key the object id, the Space id, the object type and the
//! vote kind (34 bytes, the Postgres unique key). Score the net tally, `positive - negative`, as an
//! f64. Value 24 bytes, little-endian: `positive` i64, `negative` i64, then `updated_at` in
//! microseconds since the epoch as an i64.
//!
//! The vote transaction upserts absolute tallies, so each row is complete and last-write-wins; the
//! mirror runs after the commit and never fails a batch (see `geodb_cursor::derived`). `updated_at`
//! is the wall clock at the mirror write, which is within the batch's time of Postgres' own. A
//! snapshot of the whole table at start repairs any put the best-effort mirror dropped and
//! backfills the rows that predate it. Rows are never deleted: nothing deletes from `votes_count`.

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `votes_count` in geo-db's collection registry.
pub const COLLECTION: u32 = 26;

/// The label the mirror's metrics carry.
pub const TABLE: &str = "votes_count";

/// The SQL that reads the whole table in the shape [`Count`] takes.
pub const SELECT_SQL: &str = "SELECT object_id, space_id, object_type, vote_kind, positive, \
    negative, (extract(epoch FROM updated_at)::numeric * 1000000)::bigint AS updated_us \
    FROM votes_count";

/// One row of `votes_count`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Count {
    pub object_id: Uuid,
    pub space_id: Uuid,
    pub object_type: i16,
    pub vote_kind: i16,
    pub positive: i64,
    pub negative: i64,
    pub updated_us: i64,
}

/// A row's geo-db key: the object id, the Space id, the object type, then the vote kind.
pub fn key(c: &Count) -> Vec<u8> {
    let mut k = Vec::with_capacity(34);
    k.extend_from_slice(c.object_id.as_bytes());
    k.extend_from_slice(c.space_id.as_bytes());
    k.push(c.object_type as u8);
    k.push(c.vote_kind as u8);
    k
}

/// The geo-db row for one count.
pub fn row(c: &Count) -> DerivedRow {
    let mut v = Vec::with_capacity(24);
    v.extend_from_slice(&c.positive.to_le_bytes());
    v.extend_from_slice(&c.negative.to_le_bytes());
    v.extend_from_slice(&c.updated_us.to_le_bytes());
    DerivedRow {
        collection: COLLECTION,
        group: c.object_id.as_u128(),
        score: (c.positive as f64) - (c.negative as f64),
        key: key(c),
        value: v,
    }
}

/// Sends `counts` as puts. Never fails.
pub async fn mirror(mirror: &DerivedMirror, counts: &[Count]) {
    let puts: Vec<DerivedRow> = counts.iter().map(row).collect();
    if !puts.is_empty() {
        mirror.put(TABLE, &puts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn count() -> Count {
        Count {
            object_id: u(3),
            space_id: u(7),
            object_type: 0,
            vote_kind: 1,
            positive: 5,
            negative: 2,
            updated_us: 1_793_000_000_000_000,
        }
    }

    #[test]
    fn a_count_is_grouped_by_object_and_keyed_by_object_space_type_kind() {
        let r = row(&count());
        assert_eq!(r.collection, 26);
        assert_eq!(r.group, u(3).as_u128());
        assert_eq!(r.key.len(), 34);
        assert_eq!(&r.key[0..16], &[3u8; 16]);
        assert_eq!(&r.key[16..32], &[7u8; 16]);
        assert_eq!(r.key[32], 0);
        assert_eq!(r.key[33], 1);
    }

    #[test]
    fn the_score_is_the_net_tally() {
        assert_eq!(row(&count()).score, 3.0);
        let down = Count {
            positive: 1,
            negative: 4,
            ..count()
        };
        assert_eq!(row(&down).score, -3.0);
    }

    #[test]
    fn the_value_holds_positive_negative_and_updated_at() {
        let v = row(&count()).value;
        assert_eq!(v.len(), 24);
        assert_eq!(i64::from_le_bytes(v[0..8].try_into().unwrap()), 5);
        assert_eq!(i64::from_le_bytes(v[8..16].try_into().unwrap()), 2);
        assert_eq!(
            i64::from_le_bytes(v[16..24].try_into().unwrap()),
            1_793_000_000_000_000
        );
    }

    #[test]
    fn two_kinds_of_one_object_are_two_keys() {
        let other = Count {
            vote_kind: 2,
            ..count()
        };
        assert_ne!(key(&count()), key(&other));
    }
}
