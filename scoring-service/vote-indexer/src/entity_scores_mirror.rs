//! Mirrors the engaged rows of `public.entity_ranking_scores` into geo-db's derived class (plan
//! 10.14, 10.21 section 9), collection 21.
//!
//! Collection 21 `entity_ranking_scores` is an **engaged-only overlay**, not the table: group 0,
//! key the entity id (16 bytes), score the stored `ranking_score` as the double the feed orders by,
//! value the 62-byte [`crate::entity_scores_default::Overlay`] (vote, stance and commenter counts,
//! watch, age-credit and trending terms, trending source and topic). A row exists only while some
//! engagement term of the entity is nonzero; an absent key means the un-engaged entity, whose row
//! a reader builds from the card and config ([`crate::entity_scores_default::default_row`]). The
//! whole table is 48.9M rows (about 7 GB resident), the engaged part about 9.5k.
//!
//! `refresh_entity_ranking_scores` is an upsert that never deletes, so after the call the mirror
//! reads the rows of the touched entity ids and [`plan`] puts each engaged one and deletes the key
//! of each touched id whose row is absent or no longer engaged (an entity that fell back to zero).
//! Last write wins, so an unchanged row is harmless; a delete of an absent key is a no-op. The
//! `api/scripts` backfills are SQL and are not mirrored. It never fails the caller (see
//! `geodb_cursor::derived`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::entity_scores_default::{Engagement, Overlay, TrendingSource, OVERLAY_LEN};

/// `entity_ranking_scores` in geo-db's collection registry. The one place the number is written.
pub const COLLECTION: u32 = 21;

/// The value's length in bytes.
pub const VALUE_LEN: usize = OVERLAY_LEN;

/// The SQL that reads the rows for a list of entity ids, in the shape [`ScoreRow`] takes.
pub const SELECT_SQL: &str = "SELECT entity_id, ranking_score::float8 AS ranking_score, \
    positive, negative, stance_positive, stance_negative, commenter_count, \
    participation_score::float8 AS participation_score, comment_score::float8 AS comment_score, \
    topic_score::float8 AS topic_score, watch_score::float8 AS watch_score, \
    age_credit::float8 AS age_credit, trending_score::float8 AS trending_score, \
    trending_source, trending_topic_id \
    FROM public.entity_ranking_scores WHERE entity_id = ANY($1)";

/// One row of `public.entity_ranking_scores`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreRow {
    pub entity_id: Uuid,
    pub ranking_score: f64,
    pub positive: i64,
    pub negative: i64,
    pub stance_positive: i64,
    pub stance_negative: i64,
    pub commenter_count: i64,
    pub participation_score: f64,
    pub comment_score: f64,
    pub topic_score: f64,
    pub watch_score: f64,
    pub age_credit: f64,
    pub trending_score: f64,
    pub trending_source: Option<String>,
    pub trending_topic_id: Option<Uuid>,
}

impl ScoreRow {
    fn engagement(&self) -> Engagement {
        Engagement {
            positive: self.positive,
            negative: self.negative,
            stance_positive: self.stance_positive,
            stance_negative: self.stance_negative,
            commenter_count: self.commenter_count,
            participation_score: self.participation_score,
            comment_score: self.comment_score,
            topic_score: self.topic_score,
            watch_score: self.watch_score,
            age_credit: self.age_credit,
            trending_score: self.trending_score,
            trending_source: match self.trending_source.as_deref() {
                Some("topic") => Some(TrendingSource::Topic),
                Some(_) => Some(TrendingSource::Listing),
                None => None,
            },
            trending_topic_id: self.trending_topic_id,
        }
    }
}

fn finite(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// What to send for one batch: engaged rows to put, keys that left the overlay to delete.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub puts: Vec<DerivedRow>,
    pub deletes: Vec<Vec<u8>>,
}

/// The plan for `touched` entity ids given the rows Postgres now holds for them. An id with an
/// engaged row is put; an id with no row or an un-engaged one is deleted.
pub fn plan(touched: &[Uuid], rows: &[ScoreRow]) -> Plan {
    let mut out = Plan::default();
    for id in touched {
        match rows.iter().find(|r| r.entity_id == *id) {
            Some(r) if r.engagement().is_engaged() => out.puts.push(DerivedRow {
                collection: COLLECTION,
                group: 0,
                score: finite(r.ranking_score),
                key: id.as_bytes().to_vec(),
                value: Overlay::from_row(r.ranking_score, &r.engagement()).encode(),
            }),
            _ => out.deletes.push(id.as_bytes().to_vec()),
        }
    }
    out
}

fn map_row(r: &sqlx::postgres::PgRow) -> ScoreRow {
    ScoreRow {
        entity_id: r.get("entity_id"),
        ranking_score: r.get("ranking_score"),
        positive: r.get("positive"),
        negative: r.get("negative"),
        stance_positive: r.get("stance_positive"),
        stance_negative: r.get("stance_negative"),
        commenter_count: r.get("commenter_count"),
        participation_score: r.get("participation_score"),
        comment_score: r.get("comment_score"),
        topic_score: r.get("topic_score"),
        watch_score: r.get("watch_score"),
        age_credit: r.get("age_credit"),
        trending_score: r.get("trending_score"),
        trending_source: r.get("trending_source"),
        trending_topic_id: r.get("trending_topic_id"),
    }
}

/// Reads the rows for `entity_ids`.
pub async fn read_rows(pool: &PgPool, entity_ids: &[Uuid]) -> Result<Vec<ScoreRow>, sqlx::Error> {
    let rows = sqlx::query(SELECT_SQL)
        .bind(entity_ids)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(map_row).collect())
}

/// The SQL for one keyset page of the engaged rows: the same columns as [`SELECT_SQL`], filtered
/// by the negation of the un-engaged row of section 9.1 (the SQL twin of
/// `Engagement::is_engaged`), ordered by entity id, after `$1`, at most `$2` rows.
pub const ENGAGED_PAGE_SQL: &str = "SELECT entity_id, ranking_score::float8 AS ranking_score, \
    positive, negative, stance_positive, stance_negative, commenter_count, \
    participation_score::float8 AS participation_score, comment_score::float8 AS comment_score, \
    topic_score::float8 AS topic_score, watch_score::float8 AS watch_score, \
    age_credit::float8 AS age_credit, trending_score::float8 AS trending_score, \
    trending_source, trending_topic_id \
    FROM public.entity_ranking_scores WHERE entity_id > $1 AND (positive <> 0 OR negative <> 0 \
    OR stance_positive <> 0 OR stance_negative <> 0 OR commenter_count <> 0 \
    OR participation_score <> 0 OR comment_score <> 0 OR topic_score <> 0 OR watch_score <> 0 \
    OR age_credit <> 0 OR trending_score <> 0 OR trending_source IS NOT NULL) \
    ORDER BY entity_id LIMIT $2";

/// One page of engaged rows with entity id after `after` (the nil id starts), in id order.
pub async fn engaged_page(
    pool: &PgPool,
    after: Uuid,
    limit: i64,
) -> Result<Vec<ScoreRow>, sqlx::Error> {
    let rows = sqlx::query(ENGAGED_PAGE_SQL)
        .bind(after)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(map_row).collect())
}

/// The puts for a page of engaged rows. A row the SQL filter let through but [`plan`] would delete
/// (the two predicates disagreeing, e.g. a NaN) is dropped, never sent as a delete.
pub fn engaged_puts(rows: &[ScoreRow]) -> Vec<DerivedRow> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.entity_id).collect();
    plan(&ids, rows).puts
}

/// Sends the plan for `touched`. Never fails.
pub async fn mirror(mirror: &DerivedMirror, touched: &[Uuid], rows: &[ScoreRow]) {
    let Plan { puts, deletes } = plan(touched, rows);
    if !puts.is_empty() {
        mirror.put("entity_ranking_scores", &puts).await;
    }
    if !deletes.is_empty() {
        mirror
            .delete("entity_ranking_scores", COLLECTION, &deletes)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u8, ranking: f64) -> ScoreRow {
        ScoreRow {
            entity_id: Uuid::from_bytes([n; 16]),
            ranking_score: ranking,
            positive: 7,
            negative: 2,
            stance_positive: 1,
            stance_negative: 0,
            commenter_count: 3,
            participation_score: 4.5,
            comment_score: 1.0,
            topic_score: 0.0,
            watch_score: 0.25,
            age_credit: 0.0,
            trending_score: 0.0,
            trending_source: None,
            trending_topic_id: None,
        }
    }

    fn quiet(n: u8, ranking: f64) -> ScoreRow {
        ScoreRow {
            positive: 0,
            negative: 0,
            stance_positive: 0,
            commenter_count: 0,
            participation_score: 0.0,
            comment_score: 0.0,
            watch_score: 0.0,
            ..row(n, ranking)
        }
    }

    #[test]
    fn the_collection_is_21_and_the_value_fits_the_derived_class() {
        assert_eq!(COLLECTION, 21);
        const { assert!(VALUE_LEN <= geodb_cursor::derived::VALUE_MAX) };
    }

    #[test]
    fn an_engaged_row_is_put_keyed_by_entity_scored_by_ranking() {
        let id = Uuid::from_bytes([3; 16]);
        let p = plan(&[id], &[row(3, 17885.66401799888)]);
        assert!(p.deletes.is_empty());
        let r = &p.puts[0];
        assert_eq!((r.collection, r.group), (21, 0));
        assert_eq!(r.key, vec![3u8; 16]);
        assert_eq!(r.score, 17885.66401799888);
        assert_eq!(r.value.len(), 62);
        assert_eq!(&r.value[2..6], &7u32.to_le_bytes());
        assert_eq!(&r.value[22..30], &0.25f64.to_le_bytes());
        assert!(r.put_line().is_ok());
    }

    #[test]
    fn engaged_puts_keeps_the_engaged_rows_and_never_deletes() {
        let rows = [row(1, 5.0), quiet(2, 4.0), row(3, 6.0)];
        let puts = engaged_puts(&rows);
        assert_eq!(puts.len(), 2);
        assert_eq!(puts[0].key, vec![1u8; 16]);
        assert_eq!(puts[1].key, vec![3u8; 16]);
    }

    #[test]
    fn the_page_sql_names_every_engagement_term() {
        for col in [
            "positive",
            "negative",
            "stance_positive",
            "stance_negative",
            "commenter_count",
            "participation_score",
            "comment_score",
            "topic_score",
            "watch_score",
            "age_credit",
            "trending_score",
            "trending_source IS NOT NULL",
        ] {
            assert!(
                ENGAGED_PAGE_SQL.contains(&format!("{col} <> 0")) || col.contains("IS NOT"),
                "{col}"
            );
        }
        assert!(ENGAGED_PAGE_SQL.contains("ORDER BY entity_id LIMIT"));
    }

    #[test]
    fn an_unengaged_row_and_a_missing_row_are_deleted() {
        let (a, b, c) = (
            Uuid::from_bytes([1; 16]),
            Uuid::from_bytes([2; 16]),
            Uuid::from_bytes([3; 16]),
        );
        let p = plan(&[a, b, c], &[row(1, 5.0), quiet(2, 4.0)]);
        assert_eq!(p.puts.len(), 1);
        assert_eq!(p.deletes, vec![vec![2u8; 16], vec![3u8; 16]]);
    }

    #[test]
    fn a_topic_term_alone_is_engagement() {
        let mut r = quiet(5, 9.0);
        r.topic_score = 0.5;
        assert_eq!(plan(&[r.entity_id], &[r]).puts.len(), 1);
    }

    #[test]
    fn trending_source_maps_listing_and_topic() {
        let mut r = quiet(5, 9.0);
        r.trending_score = 1.0;
        r.trending_source = Some("topic".into());
        r.trending_topic_id = Some(Uuid::from_bytes([9; 16]));
        let v = plan(&[r.entity_id], &[r.clone()]).puts[0].value.clone();
        assert_eq!(v[1], 2);
        assert_eq!(&v[46..62], &[9u8; 16]);
        r.trending_source = Some("listing".into());
        r.trending_topic_id = None;
        let v = plan(&[r.entity_id], &[r]).puts[0].value.clone();
        assert_eq!((v[1], &v[46..62]), (1, &[0u8; 16][..]));
    }

    #[test]
    fn a_non_finite_score_is_zero_and_nothing_sends_nothing() {
        assert_eq!(
            plan(&[Uuid::from_bytes([1; 16])], &[row(1, f64::NAN)]).puts[0].score,
            0.0
        );
        assert_eq!(plan(&[], &[]), Plan::default());
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
        let id = Uuid::from_bytes([1; 16]);
        mirror(&m, &[id, Uuid::from_bytes([2; 16])], &[row(1, 1.0)]).await;
    }
}
