//! Mirrors `scoring_topology_distances` into geo-db's derived class (plan 10.14, collection 24).
//!
//! Collection 24 `scoring_topology_distances`: one row per Space, its distance from the root Space.
//! Group is 0 (the readers load the whole map, `_fetch_scoring_topology_distances` in the scoring
//! service, so there is one group). Key is the Space's uuid (16 bytes). Score is the distance as an
//! f64, so a scan walks the farthest Spaces first. Value is the distance as a little-endian i32 (four
//! bytes), the same integer Postgres holds.
//!
//! The table is tiny (101 rows measured 2026-11-06, against `derived_rows_max` 4,194,304). Postgres
//! stays the source of truth: [`plan`] turns one applied change set into puts and deletes after its
//! transaction commits, and [`snapshot`] copies the whole table once at start, which repairs any
//! put the best-effort mirror dropped. Rows are last-write-wins. Nothing here fails the indexer (see
//! `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use uuid::Uuid;

/// `scoring_topology_distances` in geo-db's collection registry.
pub const COLLECTION: u32 = 24;

/// The label the mirror's metrics carry.
pub const TABLE: &str = "scoring_topology_distances";

/// The SQL that reads the whole table.
pub const SELECT_SQL: &str = "SELECT space_id, distance FROM scoring_topology_distances";

/// One mirrored row: a Space and its distance from the root.
pub fn row(space_id: Uuid, distance: i32) -> DerivedRow {
    DerivedRow {
        collection: COLLECTION,
        group: 0,
        score: f64::from(distance),
        key: space_id.as_bytes().to_vec(),
        value: distance.to_le_bytes().to_vec(),
    }
}

/// The puts and the keys to delete for one applied change set. `Storage::apply_changes` upserts
/// first and deletes after, so an id in both lists ends deleted and gets no put; an id upserted
/// twice keeps its last distance.
pub fn plan(upserts: &[(Uuid, i32)], deletes: &[Uuid]) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
    let gone: HashSet<Uuid> = deletes.iter().copied().collect();
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut puts: Vec<DerivedRow> = Vec::new();
    for (id, distance) in upserts.iter().rev() {
        if !gone.contains(id) && seen.insert(*id) {
            puts.push(row(*id, *distance));
        }
    }
    puts.reverse();
    let mut keys: Vec<Vec<u8>> = gone.iter().map(|id| id.as_bytes().to_vec()).collect();
    keys.sort();
    (puts, keys)
}

/// Sends one change set. Never fails.
pub async fn mirror(mirror: &DerivedMirror, upserts: &[(Uuid, i32)], deletes: &[Uuid]) {
    let (puts, keys) = plan(upserts, deletes);
    if !puts.is_empty() {
        mirror.put(TABLE, &puts).await;
    }
    if !keys.is_empty() {
        mirror.delete(TABLE, COLLECTION, &keys).await;
    }
}

/// Sends the whole table as puts. Never fails.
pub async fn snapshot(mirror: &DerivedMirror, rows: &[(Uuid, i32)]) {
    let puts: Vec<DerivedRow> = rows.iter().map(|(id, d)| row(*id, *d)).collect();
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

    #[test]
    fn a_space_is_one_row_keyed_by_its_id_and_scored_by_its_distance() {
        let r = row(u(3), 4);
        assert_eq!(r.collection, 24);
        assert_eq!(r.group, 0);
        assert_eq!(r.score, 4.0);
        assert_eq!(r.key, vec![3u8; 16]);
        assert_eq!(r.value, 4i32.to_le_bytes().to_vec());
    }

    #[test]
    fn the_root_is_distance_zero() {
        let (puts, deletes) = plan(&[(u(1), 0)], &[]);
        assert!(deletes.is_empty());
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].score, 0.0);
        assert_eq!(puts[0].value, vec![0, 0, 0, 0]);
    }

    #[test]
    fn a_removed_space_is_a_delete_and_not_a_put() {
        let (puts, deletes) = plan(&[(u(1), 0), (u(2), 1)], &[u(2)]);
        assert_eq!(puts.len(), 1);
        assert_eq!(puts[0].key, vec![1u8; 16]);
        assert_eq!(deletes, vec![vec![2u8; 16]]);
    }

    #[test]
    fn a_space_moved_twice_keeps_its_last_distance_once() {
        let (puts, _) = plan(&[(u(1), 0), (u(2), 1), (u(2), 3)], &[]);
        assert_eq!(puts.len(), 2);
        let two = puts.iter().find(|p| p.key == vec![2u8; 16]).unwrap();
        assert_eq!(two.score, 3.0);
    }

    #[test]
    fn a_repeated_delete_is_sent_once() {
        let (_, deletes) = plan(&[], &[u(2), u(2)]);
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn nothing_changed_sends_nothing() {
        let (puts, deletes) = plan(&[], &[]);
        assert!(puts.is_empty() && deletes.is_empty());
    }
}
