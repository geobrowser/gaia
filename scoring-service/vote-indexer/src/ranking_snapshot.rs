//! One-shot copy of `entity_type_ranking` and `entity_topic_ranking` into geo-db's derived
//! collections 0 and 1 (RUNBOOK section 12 step 4).
//!
//! WHY A SEPARATE BINARY. The live mirror (`derived_scores`) reads the touched entities' rows
//! before and after `refresh_entity_ranking_scores` and sends a row only when its score changed,
//! so running a refresh over the whole entity set copies nothing for rows that did not move. The
//! rows that predate the flag therefore never reach geo-db. This reads both tables whole, in
//! keyset pages, and sends every row through the same put path (`derived_scores::derived_row`
//! and `DerivedMirror::put_counted`). It is the same idea as `mirror_votes_counts_snapshot`, but
//! the tables are ~2.8M rows rather than thousands, so it runs as its own Job and not at pod start.
//!
//! IDEMPOTENT. A derived row is a keyed cell and the put is last-write-wins, so a re-run, or a
//! run that overlaps live mirror writes, converges on Postgres' current value. A page holds
//! [`PAGE_SIZE`] rows, one batch request (`geodb_cursor::derived::BATCH_MAX`) when
//! `GEODB_DERIVED_BATCH=1`, so the mirror's per-call time budget is never the limit.
//!
//! Nothing here logs an entity, topic or type id: progress and results are counts.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::derived_scores::{topic_row, type_row, RankRow};

/// Rows a page reads and sends. Equal to `geodb_cursor::derived::BATCH_MAX`, so one page is one
/// batch request.
pub const PAGE_SIZE: usize = geodb_cursor::derived::BATCH_MAX;

const TYPE_FIRST: &str = "SELECT type_id, entity_id, ranking_score::float8 AS s \
    FROM entity_type_ranking ORDER BY type_id, entity_id LIMIT $1";
const TYPE_NEXT: &str = "SELECT type_id, entity_id, ranking_score::float8 AS s \
    FROM entity_type_ranking WHERE (type_id, entity_id) > ($2, $3) \
    ORDER BY type_id, entity_id LIMIT $1";
const TOPIC_FIRST: &str = "SELECT topic_id, type_id, entity_id, ranking_score::float8 AS s \
    FROM entity_topic_ranking ORDER BY topic_id, type_id, entity_id LIMIT $1";
const TOPIC_NEXT: &str = "SELECT topic_id, type_id, entity_id, ranking_score::float8 AS s \
    FROM entity_topic_ranking WHERE (topic_id, type_id, entity_id) > ($2, $3, $4) \
    ORDER BY topic_id, type_id, entity_id LIMIT $1";

/// The last primary key a page read, to resume after.
pub type TypeCursor = (Uuid, Uuid);
pub type TopicCursor = (Uuid, Uuid, Uuid);

/// The page of `entity_type_ranking` rows after `after` (the first page when `None`), in key order.
pub async fn type_page(
    pool: &PgPool,
    after: Option<TypeCursor>,
) -> Result<Vec<RankRow>, sqlx::Error> {
    let limit = PAGE_SIZE as i64;
    let rows = match after {
        None => sqlx::query(TYPE_FIRST).bind(limit).fetch_all(pool).await?,
        Some((t, e)) => {
            sqlx::query(TYPE_NEXT)
                .bind(limit)
                .bind(t)
                .bind(e)
                .fetch_all(pool)
                .await?
        }
    };
    Ok(rows
        .iter()
        .map(|r| type_row(r.get("type_id"), r.get("entity_id"), r.get("s")))
        .collect())
}

/// The page of `entity_topic_ranking` rows after `after` (the first page when `None`), in key order.
pub async fn topic_page(
    pool: &PgPool,
    after: Option<TopicCursor>,
) -> Result<Vec<(TopicCursor, RankRow)>, sqlx::Error> {
    let limit = PAGE_SIZE as i64;
    let rows = match after {
        None => sqlx::query(TOPIC_FIRST).bind(limit).fetch_all(pool).await?,
        Some((p, t, e)) => {
            sqlx::query(TOPIC_NEXT)
                .bind(limit)
                .bind(p)
                .bind(t)
                .bind(e)
                .fetch_all(pool)
                .await?
        }
    };
    Ok(rows
        .iter()
        .map(|r| {
            let (p, t, e): (Uuid, Uuid, Uuid) =
                (r.get("topic_id"), r.get("type_id"), r.get("entity_id"));
            ((p, t, e), topic_row(p, t, e, r.get("s")))
        })
        .collect())
}

/// The cursor a type page ends at: the last row's `(type_id, entity_id)`, read back from its key.
pub fn type_cursor(row: &RankRow) -> TypeCursor {
    let k = &row.key;
    (
        Uuid::from_slice(&k[0..16]).expect("type key holds a 16-byte type id"),
        Uuid::from_slice(&k[16..32]).expect("type key holds a 16-byte entity id"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derived_scores::{derived_row, TOPIC_COLLECTION, TYPE_COLLECTION};

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn a_page_is_one_batch_request() {
        assert_eq!(PAGE_SIZE, 256);
    }

    #[test]
    fn the_snapshot_row_is_the_live_mirrors_row() {
        let r = derived_row(&type_row(u(1), u(2), 0.5));
        assert_eq!(r.collection, TYPE_COLLECTION);
        assert_eq!(r.group, u(1).as_u128());
        assert_eq!(r.key.len(), 32);
        assert_eq!(r.score, 0.5);
        assert!(r.value.is_empty());

        let t = derived_row(&topic_row(u(1), u(2), u(3), 1.5));
        assert_eq!(t.collection, TOPIC_COLLECTION);
        assert_eq!(t.group, u(1).as_u128());
        assert_eq!(t.key.len(), 48);
    }

    #[test]
    fn the_type_cursor_round_trips_the_key() {
        assert_eq!(type_cursor(&type_row(u(4), u(9), 0.0)), (u(4), u(9)));
    }

    #[test]
    fn next_pages_bind_the_whole_primary_key() {
        assert!(TYPE_NEXT.contains("($2, $3)"));
        assert!(TOPIC_NEXT.contains("($2, $3, $4)"));
        assert!(TYPE_NEXT.contains("ORDER BY type_id, entity_id"));
        assert!(TOPIC_NEXT.contains("ORDER BY topic_id, type_id, entity_id"));
    }
}
