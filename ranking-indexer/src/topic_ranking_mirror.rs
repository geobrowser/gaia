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
use uuid::Uuid;

/// `entity_topic_ranking` in geo-db's collection registry.
pub const COLLECTION: u32 = 1;

/// The `table` label every mirror call carries (see `GEODB_DERIVED_MIRROR_TABLES`).
pub const TABLE: &str = "entity_topic_ranking";

/// The SQL that reads the table in the shape [`Diff::before`] and [`Diff::after`] take.
pub const SELECT_SQL: &str =
    "SELECT topic_id, type_id, entity_id, ranking_score::float8 FROM public.entity_topic_ranking";

type Key = (Uuid, Uuid, Uuid);

/// A row's geo-db key: topic, type, entity ids.
pub fn key(topic: Uuid, type_id: Uuid, entity: Uuid) -> Vec<u8> {
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
            .map(|(t, k, e)| key(*t, *k, *e))
            .collect();
        deletes.sort();
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

    #[test]
    fn key_is_topic_type_entity_in_48_bytes() {
        let k = key(u(1), u(2), u(3));
        assert_eq!(k.len(), 48);
        assert_eq!(&k[..16], u(1).as_bytes());
        assert_eq!(&k[16..32], u(2).as_bytes());
        assert_eq!(&k[32..], u(3).as_bytes());
    }

    #[test]
    fn inserts_score_moves_and_departures_are_planned_and_unchanged_rows_are_not() {
        let mut d = Diff::default();
        d.before(u(1), u(2), u(3), 1.0); // unchanged
        d.before(u(1), u(2), u(4), 1.0); // score moves
        d.before(u(1), u(2), u(5), 1.0); // leaves
        d.after(u(1), u(2), u(3), 1.0);
        d.after(u(1), u(2), u(4), 2.0);
        d.after(u(1), u(2), u(6), 3.0); // new
        assert_eq!(d.after_rows(), 3);
        let (puts, deletes) = d.finish();
        let keys: Vec<_> = puts.iter().map(|p| p.key.clone()).collect();
        assert_eq!(keys, vec![key(u(1), u(2), u(4)), key(u(1), u(2), u(6))]);
        assert_eq!(puts[0].score, 2.0);
        assert_eq!(puts[0].collection, 1);
        assert_eq!(puts[0].group, u(1).as_u128());
        assert_eq!(deletes, vec![key(u(1), u(2), u(5))]);
    }

    #[test]
    fn nothing_changed_sends_nothing() {
        let mut d = Diff::default();
        d.before(u(1), u(2), u(3), 0.5);
        d.after(u(1), u(2), u(3), 0.5);
        let (puts, deletes) = d.finish();
        assert!(puts.is_empty() && deletes.is_empty());
    }
}
