//! Mirrors `public.space_topic_suggestions` into geo-db's derived class (plan 10.14, collection 13).
//!
//! Collection 13 `space_topic_suggestions`: group the Space id as a big-endian `u128`, key the Space
//! id then the topic id (32 bytes; a key is unique in its collection), score minus the rank (so a
//! scan walks rank 1 first), value sixteen bytes: `links` i32, `rank` i32 and `refreshed_at` i64
//! microseconds since the epoch, all little-endian. `name_key` is not mirrored: it is the topic's
//! name normalised, and the reader has the name.
//!
//! `refresh_space_topic_suggestions()` deletes and re-inserts the whole table in one transaction,
//! so the mirror reads the whole table after the commit and sends all of it: every row as a put
//! (last write wins, so unchanged rows are harmless) and a delete for each (Space, topic) the table
//! held before and no longer has. It never fails the job (see `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `space_topic_suggestions` in geo-db's collection registry.
pub const COLLECTION: u32 = 13;

/// The SQL that reads the table in the shape [`Suggestion`] takes.
pub const SELECT_SQL: &str = "SELECT space_id, topic_id, links, rank, \
    (extract(epoch FROM refreshed_at)::numeric * 1000000)::bigint AS refreshed_us \
    FROM public.space_topic_suggestions";

/// The SQL that reads the keys the table holds now, before a refresh replaces it.
pub const PRIOR_SQL: &str = "SELECT space_id, topic_id FROM public.space_topic_suggestions";

/// One row of `public.space_topic_suggestions`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Suggestion {
    pub space_id: Uuid,
    pub topic_id: Uuid,
    pub links: i32,
    pub rank: i32,
    pub refreshed_us: i64,
}

/// A row's geo-db key: the Space id then the topic id.
pub fn key(space: Uuid, topic: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(32);
    k.extend_from_slice(space.as_bytes());
    k.extend_from_slice(topic.as_bytes());
    k
}

fn value(r: &Suggestion) -> Vec<u8> {
    let mut v = Vec::with_capacity(16);
    v.extend_from_slice(&r.links.to_le_bytes());
    v.extend_from_slice(&r.rank.to_le_bytes());
    v.extend_from_slice(&r.refreshed_us.to_le_bytes());
    v
}

/// The rows to put and the keys to delete after the refresh left `rows`, given the
/// `(space, topic)` pairs the table held before it.
pub fn plan(rows: &[Suggestion], prior: &[(Uuid, Uuid)]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: r.space_id.as_u128(),
            score: -f64::from(r.rank),
            key: key(r.space_id, r.topic_id),
            value: value(r),
        })
        .collect();
    let now: HashSet<(Uuid, Uuid)> = rows.iter().map(|r| (r.space_id, r.topic_id)).collect();
    let deletes = prior
        .iter()
        .filter(|p| !now.contains(p))
        .map(|(s, t)| key(*s, *t))
        .collect();
    (puts, deletes)
}

/// Sends the plan for one refresh. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[Suggestion], prior: &[(Uuid, Uuid)]) {
    let (puts, deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("space_topic_suggestions", &puts).await;
    }
    if !deletes.is_empty() {
        mirror
            .delete("space_topic_suggestions", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(space: u8, topic: u8, rank: i32) -> Suggestion {
        Suggestion {
            space_id: u(space),
            topic_id: u(topic),
            links: 100 - rank,
            rank,
            refreshed_us: 1_700_000_000_000_000,
        }
    }

    #[test]
    fn a_row_is_grouped_by_space_and_keyed_by_space_then_topic() {
        let (puts, deletes) = plan(&[row(1, 2, 3)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 13);
        assert_eq!(p.group, u(1).as_u128());
        assert_eq!(p.score, -3.0);
        let mut key = vec![1u8; 16];
        key.extend_from_slice(&[2u8; 16]);
        assert_eq!(p.key, key);
        assert_eq!(p.value.len(), 16);
        assert_eq!(&p.value[0..4], &97i32.to_le_bytes());
        assert_eq!(&p.value[4..8], &3i32.to_le_bytes());
        assert_eq!(&p.value[8..16], &1_700_000_000_000_000i64.to_le_bytes());
    }

    #[test]
    fn rank_one_sorts_before_rank_two_under_a_descending_score() {
        let (puts, _) = plan(&[row(1, 2, 1), row(1, 3, 2)], &[]);
        assert!(puts[0].score > puts[1].score);
    }

    #[test]
    fn pairs_the_table_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [row(1, 2, 1)];
        let (puts, deletes) = plan(&rows, &[(u(1), u(2)), (u(1), u(3)), (u(4), u(2))]);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes, vec![key(u(1), u(3)), key(u(4), u(2))]);
    }

    #[test]
    fn the_same_topic_in_two_spaces_is_two_keys() {
        let (puts, _) = plan(&[row(1, 9, 1), row(2, 9, 1)], &[]);
        assert_ne!(puts[0].key, puts[1].key);
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[(u(1), u(2))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }
}
