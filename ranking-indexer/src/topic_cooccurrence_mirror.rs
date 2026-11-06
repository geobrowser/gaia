//! Mirrors `personalization.topic_cooccurrence` into geo-db's derived class (plan 10.14, collection 23).
//!
//! Collection 23 `topic_cooccurrence`: group the topic id as a big-endian `u128`, key the topic id
//! then the neighbour id (32 bytes; a key is unique in its collection), score the `similarity`, so
//! a scan of one topic walks its closest neighbour first (the reader takes one index range per
//! topic the user has), value four bytes: `support` as a little-endian `i32`.
//!
//! `refresh_topic_cooccurrence()` deletes and re-inserts the whole table inside the nightly refit's
//! transaction, so the mirror reads the whole table after the commit and sends all of it: every row
//! as a put (last write wins, so an unchanged row is harmless) and a delete for each (topic,
//! neighbour) the table held before and no longer has. The table holds at most
//! `spread_max_neighbours` (10) rows per topic. It never fails the job (see
//! `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `topic_cooccurrence` in geo-db's collection registry. The one place the number is written.
pub const COLLECTION: u32 = 23;

/// The value's length in bytes.
pub const VALUE_LEN: usize = 4;

/// The SQL that reads the table in the shape [`Neighbour`] takes.
pub const SELECT_SQL: &str = "SELECT topic_id, neighbour_id, similarity, support \
    FROM personalization.topic_cooccurrence";

/// The SQL that reads the keys the table holds now, before a refit replaces it.
pub const PRIOR_SQL: &str = "SELECT topic_id, neighbour_id FROM personalization.topic_cooccurrence";

/// One row of `personalization.topic_cooccurrence`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Neighbour {
    pub topic_id: Uuid,
    pub neighbour_id: Uuid,
    pub similarity: f64,
    pub support: i32,
}

/// A row's geo-db key: the topic id then the neighbour id.
pub fn key(topic: Uuid, neighbour: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(32);
    k.extend_from_slice(topic.as_bytes());
    k.extend_from_slice(neighbour.as_bytes());
    k
}

/// The rows to put and the keys to delete after the refit left `rows`, given the
/// `(topic, neighbour)` pairs the table held before it.
pub fn plan(rows: &[Neighbour], prior: &[(Uuid, Uuid)]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let puts: Vec<DerivedRow> = rows
        .iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: r.topic_id.as_u128(),
            score: if r.similarity.is_finite() {
                r.similarity
            } else {
                0.0
            },
            key: key(r.topic_id, r.neighbour_id),
            value: r.support.to_le_bytes().to_vec(),
        })
        .collect();
    let now: HashSet<(Uuid, Uuid)> = rows.iter().map(|r| (r.topic_id, r.neighbour_id)).collect();
    let deletes = prior
        .iter()
        .filter(|p| !now.contains(p))
        .map(|(t, n)| key(*t, *n))
        .collect();
    (puts, deletes)
}

/// Sends the plan for one refit. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[Neighbour], prior: &[(Uuid, Uuid)]) {
    let (puts, deletes) = plan(rows, prior);
    if !puts.is_empty() {
        mirror.put("topic_cooccurrence", &puts).await;
    }
    if !deletes.is_empty() {
        mirror
            .delete("topic_cooccurrence", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn row(topic: u8, neighbour: u8, similarity: f64) -> Neighbour {
        Neighbour {
            topic_id: u(topic),
            neighbour_id: u(neighbour),
            similarity,
            support: 12,
        }
    }

    #[test]
    fn the_collection_is_23() {
        assert_eq!(COLLECTION, 23);
    }

    #[test]
    fn a_row_is_grouped_by_topic_and_keyed_by_topic_then_neighbour() {
        let (puts, deletes) = plan(&[row(1, 2, 0.75)], &[]);
        assert!(deletes.is_empty());
        let p = &puts[0];
        assert_eq!(p.collection, 23);
        assert_eq!(p.group, u(1).as_u128());
        assert_eq!(p.score, 0.75);
        let mut key = vec![1u8; 16];
        key.extend_from_slice(&[2u8; 16]);
        assert_eq!(p.key, key);
        assert_eq!(p.value, 12i32.to_le_bytes().to_vec());
        assert_eq!(p.value.len(), VALUE_LEN);
    }

    #[test]
    fn a_closer_neighbour_scores_higher() {
        let (puts, _) = plan(&[row(1, 2, 0.9), row(1, 3, 0.2)], &[]);
        assert!(puts[0].score > puts[1].score);
    }

    #[test]
    fn pairs_the_table_no_longer_has_are_deleted_and_kept_ones_are_not() {
        let rows = [row(1, 2, 0.5)];
        let (puts, deletes) = plan(&rows, &[(u(1), u(2)), (u(1), u(3)), (u(4), u(2))]);
        assert_eq!(puts.len(), 1);
        assert_eq!(deletes, vec![key(u(1), u(3)), key(u(4), u(2))]);
    }

    #[test]
    fn the_two_directions_of_a_pair_are_two_keys() {
        let (puts, _) = plan(&[row(1, 2, 0.5), row(2, 1, 0.5)], &[]);
        assert_ne!(puts[0].key, puts[1].key);
        assert_ne!(puts[0].group, puts[1].group);
    }

    #[test]
    fn a_non_finite_similarity_is_zero_not_sent_as_nan() {
        assert_eq!(plan(&[row(1, 2, f64::NAN)], &[]).0[0].score, 0.0);
    }

    #[test]
    fn an_empty_table_deletes_everything_it_held() {
        let (puts, deletes) = plan(&[], &[(u(1), u(2))]);
        assert!(puts.is_empty());
        assert_eq!(deletes.len(), 1);
    }
}
