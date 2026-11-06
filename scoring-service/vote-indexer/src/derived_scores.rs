//! Mirrors the type and topic ranking rows into geo-db's derived class (plan 10.14, V10).
//!
//! `refresh_entity_ranking_scores` writes `entity_type_ranking` and `entity_topic_ranking` in SQL,
//! so the writer cannot see the rows it wrote. [`read_rows`] reads them for the touched entity ids
//! (a primary-key-prefix lookup on `entity_id`, bounded by the id list), once before the call and
//! once after; [`plan`] diffs the two into rows to put and keys to delete; [`mirror`] hands those
//! to the held pipe. Last-write-wins, so an unchanged row is not re-sent and a lost write is
//! repaired by the next refresh of the entity.
//!
//! Key layouts (collections 0 and 1): type table, group the type id, key type id then entity id
//! (32 bytes); topic table, group the topic id, key topic, type, entity ids (48 bytes). Score is
//! `ranking_score`. Neither has a value.

use std::collections::BTreeMap;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// `entity_type_ranking` in geo-db's collection registry.
pub const TYPE_COLLECTION: u32 = 0;
/// `entity_topic_ranking`.
pub const TOPIC_COLLECTION: u32 = 1;

/// One ranking row as Postgres holds it: `(collection, key) -> (group, score)`.
#[derive(Clone, Debug, PartialEq)]
pub struct RankRow {
    pub collection: u32,
    pub group: Uuid,
    pub key: Vec<u8>,
    pub score: f64,
}

fn key(ids: &[Uuid]) -> Vec<u8> {
    ids.iter().flat_map(|u| u.as_bytes().to_vec()).collect()
}

/// The type-table row for `(type_id, entity_id)`.
pub fn type_row(type_id: Uuid, entity_id: Uuid, score: f64) -> RankRow {
    RankRow {
        collection: TYPE_COLLECTION,
        group: type_id,
        key: key(&[type_id, entity_id]),
        score,
    }
}

/// The topic-table row for `(topic_id, type_id, entity_id)`.
pub fn topic_row(topic_id: Uuid, type_id: Uuid, entity_id: Uuid, score: f64) -> RankRow {
    RankRow {
        collection: TOPIC_COLLECTION,
        group: topic_id,
        key: key(&[topic_id, type_id, entity_id]),
        score,
    }
}

/// What to send: rows whose score is new or changed, and keys that left.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub puts: Vec<RankRow>,
    pub deletes: Vec<(u32, Vec<u8>)>,
}

/// Diffs the rows before a refresh against the rows after it.
///
/// A row in `after` is put unless `before` held the same score for the same key (the mirror
/// already has it, or will on the entity's next change); a key in `before` and not in `after` is
/// deleted. Output order is by `(collection, key)`, so it is deterministic.
pub fn plan(before: Vec<RankRow>, after: Vec<RankRow>) -> Plan {
    let before: BTreeMap<(u32, Vec<u8>), f64> = before
        .into_iter()
        .map(|r| ((r.collection, r.key), r.score))
        .collect();
    let mut after_keys = std::collections::BTreeSet::new();
    let mut out = Plan::default();
    let mut after = after;
    after.sort_by(|a, b| (a.collection, &a.key).cmp(&(b.collection, &b.key)));
    for r in after {
        after_keys.insert((r.collection, r.key.clone()));
        if before.get(&(r.collection, r.key.clone())) != Some(&r.score) {
            out.puts.push(r);
        }
    }
    for k in before.keys() {
        if !after_keys.contains(k) {
            out.deletes.push(k.clone());
        }
    }
    out
}

/// Reads both tables' rows for `entity_ids`.
pub async fn read_rows(pool: &PgPool, entity_ids: &[Uuid]) -> Result<Vec<RankRow>, sqlx::Error> {
    let mut rows = Vec::new();
    for r in sqlx::query(
        "SELECT type_id, entity_id, ranking_score::float8 AS s \
         FROM entity_type_ranking WHERE entity_id = ANY($1)",
    )
    .bind(entity_ids)
    .fetch_all(pool)
    .await?
    {
        rows.push(type_row(r.get("type_id"), r.get("entity_id"), r.get("s")));
    }
    for r in sqlx::query(
        "SELECT topic_id, type_id, entity_id, ranking_score::float8 AS s \
         FROM entity_topic_ranking WHERE entity_id = ANY($1)",
    )
    .bind(entity_ids)
    .fetch_all(pool)
    .await?
    {
        rows.push(topic_row(
            r.get("topic_id"),
            r.get("type_id"),
            r.get("entity_id"),
            r.get("s"),
        ));
    }
    Ok(rows)
}

fn table(collection: u32) -> &'static str {
    if collection == TYPE_COLLECTION {
        "entity_type_ranking"
    } else {
        "entity_topic_ranking"
    }
}

/// Sends the diff of `before` and `after` to geo-db. Never fails.
pub async fn mirror(mirror: &DerivedMirror, before: Vec<RankRow>, after: Vec<RankRow>) {
    let Plan { puts, deletes } = plan(before, after);
    for coll in [TYPE_COLLECTION, TOPIC_COLLECTION] {
        let rows: Vec<DerivedRow> = puts
            .iter()
            .filter(|r| r.collection == coll)
            .map(|r| DerivedRow {
                collection: coll,
                group: u128::from_be_bytes(*r.group.as_bytes()),
                score: r.score,
                key: r.key.clone(),
                value: Vec::new(),
            })
            .collect();
        if !rows.is_empty() {
            mirror.put(table(coll), &rows).await;
        }
        let keys: Vec<Vec<u8>> = deletes
            .iter()
            .filter(|(c, _)| *c == coll)
            .map(|(_, k)| k.clone())
            .collect();
        if !keys.is_empty() {
            mirror.delete(table(coll), coll, &keys).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn key_layouts_are_32_and_48_bytes_in_id_order() {
        let t = type_row(u(1), u(2), 0.5);
        assert_eq!(t.key.len(), 32);
        assert_eq!(&t.key[..16], &[1; 16]);
        assert_eq!(&t.key[16..], &[2; 16]);
        assert_eq!(t.group, u(1));
        let p = topic_row(u(1), u(2), u(3), 0.5);
        assert_eq!(p.key.len(), 48);
        assert_eq!(&p.key[32..], &[3; 16]);
        assert_eq!(p.collection, TOPIC_COLLECTION);
    }

    #[test]
    fn plan_puts_new_and_changed_and_deletes_vanished() {
        let before = vec![
            type_row(u(1), u(9), 1.0),        // unchanged
            type_row(u(2), u(9), 1.0),        // changed
            type_row(u(3), u(9), 1.0),        // vanished
            topic_row(u(4), u(5), u(9), 2.0), // vanished
        ];
        let after = vec![
            type_row(u(1), u(9), 1.0),
            type_row(u(2), u(9), 3.0),
            type_row(u(6), u(9), 4.0), // new
        ];
        let p = plan(before, after);
        assert_eq!(
            p.puts,
            vec![type_row(u(2), u(9), 3.0), type_row(u(6), u(9), 4.0)]
        );
        assert_eq!(
            p.deletes,
            vec![
                (TYPE_COLLECTION, type_row(u(3), u(9), 0.0).key),
                (TOPIC_COLLECTION, topic_row(u(4), u(5), u(9), 0.0).key),
            ]
        );
    }

    #[test]
    fn nothing_changed_sends_nothing() {
        let rows = vec![type_row(u(1), u(9), 1.0), topic_row(u(2), u(3), u(9), 1.0)];
        assert_eq!(plan(rows.clone(), rows), Plan::default());
    }

    #[tokio::test]
    async fn mirror_with_a_dead_pipe_returns_normally() {
        let m = DerivedMirror::new(
            geodb_cursor::Config {
                program: "/nonexistent/geodb-client".into(),
                args: vec![],
                collection: 0,
                timeout: std::time::Duration::from_secs(1),
                cooldown: std::time::Duration::from_secs(60),
            },
            std::time::Duration::from_secs(1),
        );
        mirror(
            &m,
            vec![type_row(u(3), u(9), 1.0)],
            vec![type_row(u(1), u(9), 1.0)],
        )
        .await;
    }
}
