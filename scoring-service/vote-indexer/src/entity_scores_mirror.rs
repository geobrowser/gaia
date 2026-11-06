//! Mirrors `public.entity_ranking_scores` into geo-db's derived class (plan 10.14, collection 21).
//!
//! Collection 21 `entity_ranking_scores`: group 0, key the entity id (16 bytes), score the
//! `ranking_score` as the double the feed orders by (`entityOrderByRankingScorePlugin` orders by
//! `ranking_score::float8`), value 48 little-endian bytes: `quality_score`, `intrinsic_score`,
//! `type_weight` (f64 each), `positive`, `negative` (i64 each), `updated_at` (i64 microseconds
//! since the epoch). A point read serves "an entity's score or quality", a scan of group 0 serves
//! the global order, and the value carries the quality for the threshold filter.
//!
//! `refresh_entity_ranking_scores` is an upsert that never deletes, so the mirror only puts: it
//! reads the rows for the touched entity ids after the call and sends each (last write wins, so an
//! unchanged row is harmless). Only the entities a vote batch or a sweep touches are sent. The
//! `api/scripts` backfills are SQL and are not mirrored: the table is 48.9M rows and the derived
//! class refuses a put past `derived_rows_max`, so loading all of it is a sizing decision (plan
//! 10.14, V4), not something a sweep does by accident. It never fails the caller (see
//! `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// `entity_ranking_scores` in geo-db's collection registry. The one place the number is written.
pub const COLLECTION: u32 = 21;

/// The value's length in bytes.
pub const VALUE_LEN: usize = 48;

/// The SQL that reads the rows for a list of entity ids, in the shape [`ScoreRow`] takes.
pub const SELECT_SQL: &str = "SELECT entity_id, ranking_score::float8 AS ranking_score, \
    quality_score::float8 AS quality_score, intrinsic_score::float8 AS intrinsic_score, \
    type_weight::float8 AS type_weight, positive, negative, \
    (extract(epoch FROM updated_at)::numeric * 1000000)::bigint AS updated_us \
    FROM public.entity_ranking_scores WHERE entity_id = ANY($1)";

/// One row of `public.entity_ranking_scores`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ScoreRow {
    pub entity_id: Uuid,
    pub ranking_score: f64,
    pub quality_score: f64,
    pub intrinsic_score: f64,
    pub type_weight: f64,
    pub positive: i64,
    pub negative: i64,
    pub updated_us: i64,
}

fn finite(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

fn value(r: &ScoreRow) -> Vec<u8> {
    let mut v = Vec::with_capacity(VALUE_LEN);
    v.extend_from_slice(&finite(r.quality_score).to_le_bytes());
    v.extend_from_slice(&finite(r.intrinsic_score).to_le_bytes());
    v.extend_from_slice(&finite(r.type_weight).to_le_bytes());
    v.extend_from_slice(&r.positive.to_le_bytes());
    v.extend_from_slice(&r.negative.to_le_bytes());
    v.extend_from_slice(&r.updated_us.to_le_bytes());
    v
}

/// The rows to put for `rows`.
pub fn plan(rows: &[ScoreRow]) -> Vec<DerivedRow> {
    rows.iter()
        .map(|r| DerivedRow {
            collection: COLLECTION,
            group: 0,
            score: finite(r.ranking_score),
            key: r.entity_id.as_bytes().to_vec(),
            value: value(r),
        })
        .collect()
}

/// Reads the rows for `entity_ids`.
pub async fn read_rows(pool: &PgPool, entity_ids: &[Uuid]) -> Result<Vec<ScoreRow>, sqlx::Error> {
    let rows = sqlx::query(SELECT_SQL)
        .bind(entity_ids)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| ScoreRow {
            entity_id: r.get("entity_id"),
            ranking_score: r.get("ranking_score"),
            quality_score: r.get("quality_score"),
            intrinsic_score: r.get("intrinsic_score"),
            type_weight: r.get("type_weight"),
            positive: r.get("positive"),
            negative: r.get("negative"),
            updated_us: r.get("updated_us"),
        })
        .collect())
}

/// Sends the rows. Never fails.
pub async fn mirror(mirror: &DerivedMirror, rows: &[ScoreRow]) {
    let puts = plan(rows);
    if !puts.is_empty() {
        mirror.put("entity_ranking_scores", &puts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u8, ranking: f64) -> ScoreRow {
        ScoreRow {
            entity_id: Uuid::from_bytes([n; 16]),
            ranking_score: ranking,
            quality_score: 0.25,
            intrinsic_score: 0.5,
            type_weight: 1.5,
            positive: 7,
            negative: 2,
            updated_us: 1_700_000_000_000_000,
        }
    }

    #[test]
    fn the_collection_is_21() {
        assert_eq!(COLLECTION, 21);
    }

    #[test]
    fn a_row_is_keyed_by_entity_scored_by_ranking_with_a_48_byte_value() {
        let p = &plan(&[row(3, 17885.66401799888)])[0];
        assert_eq!((p.collection, p.group), (21, 0));
        assert_eq!(p.key, vec![3u8; 16]);
        assert_eq!(p.score, 17885.66401799888);
        assert_eq!(p.value.len(), VALUE_LEN);
        assert_eq!(&p.value[0..8], &0.25f64.to_le_bytes());
        assert_eq!(&p.value[8..16], &0.5f64.to_le_bytes());
        assert_eq!(&p.value[16..24], &1.5f64.to_le_bytes());
        assert_eq!(&p.value[24..32], &7i64.to_le_bytes());
        assert_eq!(&p.value[32..40], &2i64.to_le_bytes());
        assert_eq!(&p.value[40..48], &1_700_000_000_000_000i64.to_le_bytes());
        assert!(p.put_line().is_ok());
    }

    #[test]
    fn a_non_finite_score_is_zero_and_nothing_sends_nothing() {
        assert_eq!(plan(&[row(1, f64::NAN)])[0].score, 0.0);
        assert!(plan(&[]).is_empty());
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
        mirror(&m, &[row(1, 1.0)]).await;
    }
}
