//! One-shot copy of `global_scores`, `local_scores` and `space_scores` into geo-db's derived
//! collections 2, 3 and 4 (RUNBOOK section 2f-scores, "Stage B").
//!
//! The nightly scoring CronJob (`scoring-service/cronjob`, Python) is the only writer of these
//! tables. It rewrites every row each run and mirrors them best effort behind
//! `GEODB_DERIVED_MIRROR`, bounded by a per-call time budget, so a first run over ~4M rows can drop
//! most of them without failing. This reads the tables whole, in keyset pages, and sends every row
//! through `DerivedMirror::put_counted`, so a short send is seen and the binary exits non-zero.
//!
//! Row layouts are the Python mirror's (`cronjob/src/geodb_derived/__init__.py`), byte for byte:
//! `global_scores` (2) group 0, key the 16-byte entity id; `local_scores` (3) group the Space id
//! as a big-endian u128, key entity id then Space id (32 bytes); `space_scores` (4) group 0, key
//! the Space id. Score is the table's `score`; no value.
//!
//! IDEMPOTENT: last-write-wins keyed cells, so a re-run, or a run that overlaps the nightly
//! mirror, converges on Postgres' current value. Never deletes (the writer never deletes).
//!
//! Nothing here logs an entity or Space id: progress and results are counts.

use geodb_cursor::derived::DerivedRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// `global_scores` in geo-db's collection registry.
pub const GLOBAL_COLLECTION: u32 = 2;
/// `local_scores`.
pub const LOCAL_COLLECTION: u32 = 3;
/// `space_scores`.
pub const SPACE_COLLECTION: u32 = 4;

/// Rows a page reads and sends. Equal to `geodb_cursor::derived::BATCH_MAX`, so one page is one
/// batch request.
pub const PAGE_SIZE: usize = geodb_cursor::derived::BATCH_MAX;

const GLOBAL_FIRST: &str =
    "SELECT entity_id, score::float8 AS s FROM global_scores ORDER BY entity_id LIMIT $1";
const GLOBAL_NEXT: &str = "SELECT entity_id, score::float8 AS s FROM global_scores \
    WHERE entity_id > $2 ORDER BY entity_id LIMIT $1";
const LOCAL_FIRST: &str = "SELECT entity_id, space_id, score::float8 AS s FROM local_scores \
    ORDER BY entity_id, space_id LIMIT $1";
const LOCAL_NEXT: &str = "SELECT entity_id, space_id, score::float8 AS s FROM local_scores \
    WHERE (entity_id, space_id) > ($2, $3) ORDER BY entity_id, space_id LIMIT $1";
const SPACE_FIRST: &str =
    "SELECT space_id, score::float8 AS s FROM space_scores ORDER BY space_id LIMIT $1";
const SPACE_NEXT: &str = "SELECT space_id, score::float8 AS s FROM space_scores \
    WHERE space_id > $2 ORDER BY space_id LIMIT $1";

/// The last primary key a `local_scores` page read.
pub type LocalCursor = (Uuid, Uuid);

/// The `global_scores` row for `entity_id`.
pub fn global_row(entity_id: Uuid, score: f64) -> DerivedRow {
    DerivedRow {
        collection: GLOBAL_COLLECTION,
        group: 0,
        score,
        key: entity_id.as_bytes().to_vec(),
        value: Vec::new(),
    }
}

/// The `local_scores` row for `(entity_id, space_id)`.
pub fn local_row(entity_id: Uuid, space_id: Uuid, score: f64) -> DerivedRow {
    let mut key = entity_id.as_bytes().to_vec();
    key.extend_from_slice(space_id.as_bytes());
    DerivedRow {
        collection: LOCAL_COLLECTION,
        group: u128::from_be_bytes(*space_id.as_bytes()),
        score,
        key,
        value: Vec::new(),
    }
}

/// The `space_scores` row for `space_id`.
pub fn space_row(space_id: Uuid, score: f64) -> DerivedRow {
    DerivedRow {
        collection: SPACE_COLLECTION,
        group: 0,
        score,
        key: space_id.as_bytes().to_vec(),
        value: Vec::new(),
    }
}

/// A page of rows and the primary key to resume after.
pub struct Page<C> {
    pub rows: Vec<DerivedRow>,
    pub last: C,
}

/// The page of `global_scores` after `after` (the first page when `None`); `None` when exhausted.
pub async fn global_page(
    pool: &PgPool,
    after: Option<Uuid>,
) -> Result<Option<Page<Uuid>>, sqlx::Error> {
    let limit = PAGE_SIZE as i64;
    let q = match after {
        None => sqlx::query(GLOBAL_FIRST).bind(limit),
        Some(e) => sqlx::query(GLOBAL_NEXT).bind(limit).bind(e),
    };
    let rows = q.fetch_all(pool).await?;
    let Some(last) = rows.last().map(|r| r.get::<Uuid, _>("entity_id")) else {
        return Ok(None);
    };
    let rows = rows
        .iter()
        .map(|r| global_row(r.get("entity_id"), r.get("s")))
        .collect();
    Ok(Some(Page { rows, last }))
}

/// The page of `local_scores` after `after`; `None` when exhausted.
pub async fn local_page(
    pool: &PgPool,
    after: Option<LocalCursor>,
) -> Result<Option<Page<LocalCursor>>, sqlx::Error> {
    let limit = PAGE_SIZE as i64;
    let q = match after {
        None => sqlx::query(LOCAL_FIRST).bind(limit),
        Some((e, s)) => sqlx::query(LOCAL_NEXT).bind(limit).bind(e).bind(s),
    };
    let rows = q.fetch_all(pool).await?;
    let Some(last) = rows
        .last()
        .map(|r| (r.get::<Uuid, _>("entity_id"), r.get::<Uuid, _>("space_id")))
    else {
        return Ok(None);
    };
    let rows = rows
        .iter()
        .map(|r| local_row(r.get("entity_id"), r.get("space_id"), r.get("s")))
        .collect();
    Ok(Some(Page { rows, last }))
}

/// The page of `space_scores` after `after`; `None` when exhausted.
pub async fn space_page(
    pool: &PgPool,
    after: Option<Uuid>,
) -> Result<Option<Page<Uuid>>, sqlx::Error> {
    let limit = PAGE_SIZE as i64;
    let q = match after {
        None => sqlx::query(SPACE_FIRST).bind(limit),
        Some(s) => sqlx::query(SPACE_NEXT).bind(limit).bind(s),
    };
    let rows = q.fetch_all(pool).await?;
    let Some(last) = rows.last().map(|r| r.get::<Uuid, _>("space_id")) else {
        return Ok(None);
    };
    let rows = rows
        .iter()
        .map(|r| space_row(r.get("space_id"), r.get("s")))
        .collect();
    Ok(Some(Page { rows, last }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn a_page_is_one_batch_request() {
        assert_eq!(PAGE_SIZE, 256);
    }

    #[test]
    fn global_row_matches_the_python_mirror() {
        let r = global_row(u(1), 0.5);
        assert_eq!((r.collection, r.group, r.score), (2, 0, 0.5));
        assert_eq!(r.key, u(1).as_bytes().to_vec());
        assert!(r.value.is_empty());
        assert!(r.put_line().is_ok());
    }

    #[test]
    fn local_row_is_grouped_by_space_and_keyed_entity_then_space() {
        let r = local_row(u(1), u(2), 1.25);
        assert_eq!(r.collection, 3);
        assert_eq!(r.group, u(2).as_u128());
        assert_eq!(r.key.len(), 32);
        assert_eq!(&r.key[..16], u(1).as_bytes());
        assert_eq!(&r.key[16..], u(2).as_bytes());
        assert!(r.put_line().is_ok());
    }

    #[test]
    fn space_row_matches_the_python_mirror() {
        let r = space_row(u(3), 2.0);
        assert_eq!((r.collection, r.group, r.score), (4, 0, 2.0));
        assert_eq!(r.key, u(3).as_bytes().to_vec());
    }

    #[test]
    fn next_pages_bind_the_whole_primary_key() {
        assert!(LOCAL_NEXT.contains("(entity_id, space_id) > ($2, $3)"));
        assert!(LOCAL_NEXT.contains("ORDER BY entity_id, space_id"));
        assert!(GLOBAL_NEXT.contains("entity_id > $2"));
        assert!(SPACE_NEXT.contains("space_id > $2"));
    }
}
