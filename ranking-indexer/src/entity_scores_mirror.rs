//! Mirrors the `entity_ranking_scores` rows the hourly agenda refresh re-scores into geo-db's derived
//! collection 21, the engaged-only overlay (plan 10.14, 10.21 section 9).
//!
//! Collection 21 is fed by the vote-indexer (`vote_indexer::entity_scores_mirror`), which reads back the rows it
//! touched. `trending.refresh_agenda_listings()` also calls `public.refresh_entity_ranking_scores` (migrations 0113,
//! 0118) and moves the trending, age-credit and topic terms of every listed claim, its debates and the entities on a
//! listed topic, with no read-back, so those rows drifted from the overlay: the ranking-scores shadow counted them
//! `mismatch`. On the first run of each New York day (the full pass) the decaying boosts of every listed claim move.
//!
//! WHICH ROWS. The table is 48.9M rows, so a scan by engagement is out; but every row the refresh can move has a
//! nonzero trending term before or after (the partial index `entity_ranking_scores_trending_idx`, predicate
//! `trending_score <> 0 OR age_credit <> 0`, about the listed claims and their debates). The job reads the ids of that
//! set before the refresh and its full rows after the commit; a row that left the set (its boost decayed to zero) is
//! read back by primary key, since it is either still engaged through votes (a put) or now un-engaged (a delete).
//! Every row of the set is put, changed or not: last write wins, so an unchanged row is harmless and a row the mirror
//! missed earlier converges on the next run. A row whose only change is not a trending term is the vote path's.
//!
//! The value layout and the engaged predicate are the vote-indexer's (`entity_scores_default::Overlay`, 62 bytes);
//! [`Overlay::encode`] is pinned to it by a byte test. Key the entity id (16 bytes), group 0, score `ranking_score`.
//! Never fails the job (see `geodb_cursor::derived`).

use std::collections::HashSet;

use geodb_cursor::derived::{DerivedMirror, DerivedRow};
use sqlx::PgPool;
use uuid::Uuid;

/// `entity_ranking_scores` in geo-db's collection registry.
pub const COLLECTION: u32 = 21;

/// The `table` label every mirror call carries (see `GEODB_DERIVED_MIRROR_TABLES`).
pub const TABLE: &str = "entity_ranking_scores";

/// The value's length in bytes.
pub const VALUE_LEN: usize = 62;

const COLUMNS: &str = "entity_id, ranking_score::float8 AS ranking_score, \
    positive, negative, stance_positive, stance_negative, commenter_count, \
    participation_score::float8 AS participation_score, comment_score::float8 AS comment_score, \
    topic_score::float8 AS topic_score, watch_score::float8 AS watch_score, \
    age_credit::float8 AS age_credit, trending_score::float8 AS trending_score, \
    trending_source, trending_topic_id";

/// The ids with a trending or age-credit term: the partial index's predicate, so an index scan.
pub const TRENDING_IDS_SQL: &str = "SELECT entity_id FROM public.entity_ranking_scores \
    WHERE trending_score <> 0 OR age_credit <> 0";

/// The full rows of that set.
pub fn trending_rows_sql() -> String {
    format!(
        "SELECT {COLUMNS} FROM public.entity_ranking_scores WHERE trending_score <> 0 OR age_credit <> 0"
    )
}

/// The full rows of a list of ids, by primary key.
pub fn rows_by_id_sql() -> String {
    format!("SELECT {COLUMNS} FROM public.entity_ranking_scores WHERE entity_id = ANY($1)")
}

/// One row of `public.entity_ranking_scores`.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
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
    /// True when any engagement term is nonzero: the overlay's predicate (the negation of the un-engaged row).
    pub fn is_engaged(&self) -> bool {
        self.positive != 0
            || self.negative != 0
            || self.stance_positive != 0
            || self.stance_negative != 0
            || self.commenter_count != 0
            || self.participation_score != 0.0
            || self.comment_score != 0.0
            || self.topic_score != 0.0
            || self.watch_score != 0.0
            || self.age_credit != 0.0
            || self.trending_score != 0.0
            || self.trending_source.is_some()
    }

    /// The 62-byte overlay value: version 1, source (0 none, 1 listing, 2 topic), five u32 LE saturating counts, watch,
    /// age-credit and trending f64 LE, the trending topic id (zeros when none).
    pub fn encode(&self) -> Vec<u8> {
        let finite = |x: f64| if x.is_finite() { x } else { 0.0 };
        let sat = |n: i64| n.clamp(0, u32::MAX as i64) as u32;
        let mut v = Vec::with_capacity(VALUE_LEN);
        v.push(1);
        v.push(match self.trending_source.as_deref() {
            None => 0,
            Some("topic") => 2,
            Some(_) => 1,
        });
        for n in [
            self.positive,
            self.negative,
            self.stance_positive,
            self.stance_negative,
            self.commenter_count,
        ] {
            v.extend_from_slice(&sat(n).to_le_bytes());
        }
        for f in [self.watch_score, self.age_credit, self.trending_score] {
            v.extend_from_slice(&finite(f).to_le_bytes());
        }
        v.extend_from_slice(self.trending_topic_id.unwrap_or_default().as_bytes());
        v
    }
}

/// What to send: engaged rows to put, keys that left the overlay to delete.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub puts: Vec<DerivedRow>,
    pub deletes: Vec<Vec<u8>>,
}

/// The plan for `touched` ids given the rows Postgres holds for them: an id with an engaged row is put, one with no row
/// or an un-engaged one is deleted (a delete of an absent key is a no-op). Ids and keys come out sorted.
pub fn plan(touched: &HashSet<Uuid>, rows: &[ScoreRow]) -> Plan {
    let mut ids: Vec<Uuid> = touched.iter().copied().collect();
    ids.sort();
    let mut out = Plan::default();
    for id in ids {
        match rows.iter().find(|r| r.entity_id == id) {
            Some(r) if r.is_engaged() => out.puts.push(DerivedRow {
                collection: COLLECTION,
                group: 0,
                score: if r.ranking_score.is_finite() {
                    r.ranking_score
                } else {
                    0.0
                },
                key: id.as_bytes().to_vec(),
                value: r.encode(),
            }),
            _ => out.deletes.push(id.as_bytes().to_vec()),
        }
    }
    out
}

/// The ids to read before the refresh. `None` when the mirror does not take this table, so the job reads nothing.
pub async fn prior(mirror: &DerivedMirror, pool: &PgPool) -> Option<HashSet<Uuid>> {
    if !mirror.allows(TABLE) {
        return None;
    }
    match sqlx::query_scalar::<_, Uuid>(TRENDING_IDS_SQL)
        .fetch_all(pool)
        .await
    {
        Ok(ids) => Some(ids.into_iter().collect()),
        Err(e) => {
            tracing::warn!(error = %e, "entity scores mirror skipped: before-state read failed");
            None
        }
    }
}

/// After the refresh committed: read the rows, plan and send. Returns (puts, deletes) offered. Never fails the job.
pub async fn mirror(
    mirror: &DerivedMirror,
    pool: &PgPool,
    before: &HashSet<Uuid>,
) -> (usize, usize) {
    let now_rows: Vec<ScoreRow> = match sqlx::query_as(&trending_rows_sql()).fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "entity scores mirror skipped: read failed");
            return (0, 0);
        }
    };
    let mut touched: HashSet<Uuid> = now_rows.iter().map(|r| r.entity_id).collect();
    let left: Vec<Uuid> = before.difference(&touched).copied().collect();
    let mut rows = now_rows;
    if !left.is_empty() {
        match sqlx::query_as::<_, ScoreRow>(&rows_by_id_sql())
            .bind(&left)
            .fetch_all(pool)
            .await
        {
            Ok(r) => rows.extend(r),
            Err(e) => {
                tracing::warn!(error = %e, "entity scores mirror: could not read the rows that left; their keys are not touched");
                // Without the rows the ids would be read as absent and deleted: leave them out instead.
                return send(mirror, &touched, &rows).await;
            }
        }
        touched.extend(left);
    }
    send(mirror, &touched, &rows).await
}

async fn send(
    mirror: &DerivedMirror,
    touched: &HashSet<Uuid>,
    rows: &[ScoreRow],
) -> (usize, usize) {
    let Plan { puts, deletes } = plan(touched, rows);
    let counts = (puts.len(), deletes.len());
    if !puts.is_empty() {
        mirror.put(TABLE, &puts).await;
    }
    if !deletes.is_empty() {
        mirror.delete(TABLE, COLLECTION, &deletes).await;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn quiet(n: u8) -> ScoreRow {
        ScoreRow {
            entity_id: u(n),
            ranking_score: 3.5,
            positive: 0,
            negative: 0,
            stance_positive: 0,
            stance_negative: 0,
            commenter_count: 0,
            participation_score: 0.0,
            comment_score: 0.0,
            topic_score: 0.0,
            watch_score: 0.0,
            age_credit: 0.0,
            trending_score: 0.0,
            trending_source: None,
            trending_topic_id: None,
        }
    }

    fn trending(n: u8, score: f64) -> ScoreRow {
        ScoreRow {
            ranking_score: 40.0,
            age_credit: 1.5,
            trending_score: score,
            trending_source: Some("listing".into()),
            ..quiet(n)
        }
    }

    #[test]
    fn value_layout_is_the_vote_indexers() {
        // The same row the vote-indexer's `Overlay::encode` writes: 62 bytes, source topic, counts 7 2 1 0 3, terms, topic id.
        let row = ScoreRow {
            positive: 7,
            negative: 2,
            stance_positive: 1,
            commenter_count: 3,
            watch_score: 0.25,
            age_credit: 1.5,
            trending_score: 8.0,
            trending_source: Some("topic".into()),
            trending_topic_id: Some(u(9)),
            ..quiet(1)
        };
        let v = row.encode();
        assert_eq!(v.len(), VALUE_LEN);
        assert_eq!(&v[..2], &[1, 2]);
        let u32at = |at: usize| u32::from_le_bytes(v[at..at + 4].try_into().unwrap());
        assert_eq!(
            [u32at(2), u32at(6), u32at(10), u32at(14), u32at(18)],
            [7, 2, 1, 0, 3]
        );
        let f64at = |at: usize| f64::from_le_bytes(v[at..at + 8].try_into().unwrap());
        assert_eq!([f64at(22), f64at(30), f64at(38)], [0.25, 1.5, 8.0]);
        assert_eq!(&v[46..], &[9u8; 16]);
        // A listing source is 1, none is 0, and a NaN term is written as zero.
        assert_eq!(trending(1, 8.0).encode()[1], 1);
        assert_eq!(quiet(1).encode()[1], 0);
        let nan = ScoreRow {
            watch_score: f64::NAN,
            ..quiet(1)
        };
        assert_eq!(
            f64::from_le_bytes(nan.encode()[22..30].try_into().unwrap()),
            0.0
        );
    }

    #[test]
    fn counts_saturate_rather_than_wrap() {
        let row = ScoreRow {
            positive: i64::MAX,
            negative: -4,
            ..quiet(1)
        };
        let v = row.encode();
        assert_eq!(u32::from_le_bytes(v[2..6].try_into().unwrap()), u32::MAX);
        assert_eq!(u32::from_le_bytes(v[6..10].try_into().unwrap()), 0);
    }

    #[test]
    fn engaged_rows_are_put_and_rows_that_left_are_deleted() {
        // 1 still trending, 2 decayed to a quiet row, 3 decayed but still voted on, 4 has no row at all.
        let voted = ScoreRow {
            positive: 2,
            ..quiet(3)
        };
        let rows = vec![trending(1, 8.0), quiet(2), voted];
        let touched: HashSet<Uuid> = [1u8, 2, 3, 4].iter().map(|n| u(*n)).collect();
        let p = plan(&touched, &rows);
        assert_eq!(p.puts.len(), 2);
        assert_eq!(p.puts[0].key, u(1).as_bytes().to_vec());
        assert_eq!(p.puts[0].score, 40.0);
        assert_eq!(p.puts[0].value, trending(1, 8.0).encode());
        assert_eq!(p.puts[1].key, u(3).as_bytes().to_vec());
        assert_eq!(
            p.deletes,
            vec![u(2).as_bytes().to_vec(), u(4).as_bytes().to_vec()]
        );
        assert!(p
            .puts
            .iter()
            .all(|r| r.collection == COLLECTION && r.group == 0));
    }

    #[test]
    fn a_row_that_is_not_touched_is_never_sent() {
        let touched: HashSet<Uuid> = [u(1)].into_iter().collect();
        let p = plan(&touched, &[trending(1, 8.0), trending(2, 8.0)]);
        assert_eq!(p.puts.len(), 1);
        assert!(p.deletes.is_empty());
    }

    #[test]
    fn the_trending_read_matches_the_partial_index_predicate() {
        assert!(TRENDING_IDS_SQL.ends_with("WHERE trending_score <> 0 OR age_credit <> 0"));
        assert!(trending_rows_sql().ends_with("WHERE trending_score <> 0 OR age_credit <> 0"));
    }
}
