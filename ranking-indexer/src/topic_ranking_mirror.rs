//! Mirrors the rows `topic_ranking_reconcile` writes into `entity_topic_ranking` into geo-db's
//! derived collection 1 (plan 10.14, V10).
//!
//! Collection 1 is fed by the vote-indexer's `refresh_entity_ranking_scores` mirror, which reads the
//! rows it touched before and after the call. The hourly reconcile job writes the same table in SQL
//! without that read-back: `refresh_topic_debate_scores()` re-scores topic debates and
//! `reconcile_entity_topic_ranking()` inserts the rows for a tag, name or type added to an entity
//! whose score did not change, and deletes the rows of an entity that left. None of that reached
//! geo-db, so the collection drifted below Postgres (and could hold keys Postgres had dropped).
//!
//! The job reads the table before its two calls and after them, inside its transaction. [`Diff`]
//! holds the before state as `key -> score` and is fed the after state row by row, so only one
//! copy of the table is in memory; what is left over at the end is the keys that left. The plan is
//! sent after the commit. Key layout (the vote-indexer's, the one-shot snapshot's): group the topic
//! id, key topic, type, entity ids (48 bytes), score `ranking_score`, no value.

use std::collections::HashMap;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// `entity_topic_ranking` in geo-db's collection registry.
pub const COLLECTION: u32 = 1;

/// The `table` label every mirror call carries (see `GEODB_DERIVED_MIRROR_TABLES`).
pub const TABLE: &str = "entity_topic_ranking";

/// The SQL that reads the table in the shape [`Diff::before`] and [`Diff::after`] take.
pub const SELECT_SQL: &str =
    "SELECT topic_id, type_id, entity_id, ranking_score::float8 FROM public.entity_topic_ranking";

type Key = (Uuid, Uuid, Uuid);

/// The id geo-db's graph holds for a gaia entity id: a UUIDv4 as is, any other id remapped through
/// SHA-256 exactly as the feeder does (`geodb-feeder/src/ids.ts` `conformingId("entity", id)`).
/// The count's Space filter looks a candidate up in the graph by the key's entity id, so the key
/// has to carry this form, not the raw gaia id.
pub fn conforming_entity(entity: Uuid) -> Uuid {
    let b = entity.as_bytes();
    if b[6] >> 4 == 4 && matches!(b[8] >> 6, 0b10) {
        return entity;
    }
    let digest = Sha256::digest(format!("geo:legacy-entity:v1\0{}", entity.simple()).as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out[6] = (out[6] & 0x0f) | 0x40;
    out[8] = (out[8] & 0x3f) | 0x80;
    Uuid::from_bytes(out)
}

/// A row's geo-db key: topic, type, entity ids (the entity in its conformed form).
pub fn key(topic: Uuid, type_id: Uuid, entity: Uuid) -> Vec<u8> {
    key_raw(topic, type_id, conforming_entity(entity))
}

/// The key as earlier builds wrote it, with the gaia entity id unconformed. Equal to [`key`] for a v4 id.
pub fn key_raw(topic: Uuid, type_id: Uuid, entity: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(48);
    k.extend_from_slice(topic.as_bytes());
    k.extend_from_slice(type_id.as_bytes());
    k.extend_from_slice(entity.as_bytes());
    k
}

/// The before state, consumed as the after state streams past.
#[derive(Default)]
pub struct Diff {
    before: HashMap<Key, u64>,
    puts: Vec<DerivedRow>,
    /// Keys earlier builds wrote with the unconformed id, for rows put now: removed so a count
    /// does not see the row twice.
    stale_raw: Vec<Vec<u8>>,
    after_rows: usize,
}

impl Diff {
    /// Records a row of the table as it was before the writes.
    pub fn before(&mut self, topic: Uuid, type_id: Uuid, entity: Uuid, score: f64) {
        self.before
            .insert((topic, type_id, entity), score.to_bits());
    }

    /// Records a row of the table as it is after the writes: a put when it is new or its score moved.
    pub fn after(&mut self, topic: Uuid, type_id: Uuid, entity: Uuid, score: f64) {
        self.after_rows += 1;
        let was = self.before.remove(&(topic, type_id, entity));
        if was == Some(score.to_bits()) {
            return;
        }
        let raw = key_raw(topic, type_id, entity);
        if raw != key(topic, type_id, entity) {
            self.stale_raw.push(raw);
        }
        self.puts.push(DerivedRow {
            collection: COLLECTION,
            group: topic.as_u128(),
            score,
            key: key(topic, type_id, entity),
            value: Vec::new(),
        });
    }

    /// The rows to put and the keys to delete (every before row the after state did not have).
    pub fn finish(self) -> (Vec<DerivedRow>, Vec<Vec<u8>>) {
        let mut deletes: Vec<Vec<u8>> = self
            .before
            .keys()
            .flat_map(|(t, k, e)| [key(*t, *k, *e), key_raw(*t, *k, *e)])
            .chain(self.stale_raw)
            .collect();
        deletes.sort();
        deletes.dedup();
        (self.puts, deletes)
    }

    /// Rows seen in the after state.
    pub fn after_rows(&self) -> usize {
        self.after_rows
    }
}

/// Sends the plan. Never fails (see `geodb_cursor::derived`).
pub async fn mirror(mirror: &DerivedMirror, puts: &[DerivedRow], deletes: &[Vec<u8>]) {
    if !puts.is_empty() {
        mirror.put(TABLE, puts).await;
    }
    if !deletes.is_empty() {
        mirror.delete(TABLE, COLLECTION, deletes).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    /// A v4 id, which geo-db keeps as is.
    fn v4(n: u8) -> Uuid {
        let mut b = [n; 16];
        b[6] = 0x40 | (n & 0x0f);
        b[8] = 0x80 | (n & 0x3f);
        Uuid::from_bytes(b)
    }

    #[test]
    fn a_non_v4_entity_is_keyed_by_the_id_geo_db_holds_it_under() {
        // Vector from geodb-feeder/src/ids.ts conformingId("entity", ...), as in search-indexer's ids tests.
        let legacy = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let want = Uuid::parse_str("df8ff4dd-46e8-473f-8ca6-8a78442c9f2b").unwrap();
        assert_eq!(conforming_entity(legacy), want);
        assert_eq!(&key(u(1), u(2), legacy)[32..], want.as_bytes());
        assert_eq!(conforming_entity(v4(7)), v4(7));
        assert_eq!(key(u(1), u(2), v4(7)), key_raw(u(1), u(2), v4(7)));
    }

    #[test]
    fn a_non_v4_row_also_clears_its_raw_key_from_earlier_builds() {
        let mut d = Diff::default();
        d.before(u(1), u(2), u(5), 1.0); // leaves, non-v4
        d.after(u(1), u(2), u(6), 3.0); // new, non-v4
        let (puts, deletes) = d.finish();
        assert_eq!(puts[0].key, key(u(1), u(2), u(6)));
        for e in [u(5), u(6)] {
            assert!(deletes.contains(&key_raw(u(1), u(2), e)));
        }
        assert!(deletes.contains(&key(u(1), u(2), u(5))));
    }

    #[test]
    fn key_is_topic_type_entity_in_48_bytes() {
        let k = key(u(1), u(2), v4(3));
        assert_eq!(k.len(), 48);
        assert_eq!(&k[..16], u(1).as_bytes());
        assert_eq!(&k[16..32], u(2).as_bytes());
        assert_eq!(&k[32..], v4(3).as_bytes());
    }

    #[test]
    fn inserts_score_moves_and_departures_are_planned_and_unchanged_rows_are_not() {
        let mut d = Diff::default();
        d.before(u(1), u(2), v4(3), 1.0); // unchanged
        d.before(u(1), u(2), v4(4), 1.0); // score moves
        d.before(u(1), u(2), v4(5), 1.0); // leaves
        d.after(u(1), u(2), v4(3), 1.0);
        d.after(u(1), u(2), v4(4), 2.0);
        d.after(u(1), u(2), v4(6), 3.0); // new
        assert_eq!(d.after_rows(), 3);
        let (puts, deletes) = d.finish();
        let keys: Vec<_> = puts.iter().map(|p| p.key.clone()).collect();
        assert_eq!(keys, vec![key(u(1), u(2), v4(4)), key(u(1), u(2), v4(6))]);
        assert_eq!(puts[0].score, 2.0);
        assert_eq!(puts[0].collection, 1);
        assert_eq!(puts[0].group, u(1).as_u128());
        assert_eq!(deletes, vec![key(u(1), u(2), v4(5))]);
    }

    #[test]
    fn nothing_changed_sends_nothing() {
        let mut d = Diff::default();
        d.before(u(1), u(2), v4(3), 0.5);
        d.after(u(1), u(2), v4(3), 0.5);
        let (puts, deletes) = d.finish();
        assert!(puts.is_empty() && deletes.is_empty());
    }
}
