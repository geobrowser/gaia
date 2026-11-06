//! Mirrors the two one-row watermark tables into geo-db's derived class (plan 10.14 section 5,
//! durable-state collections): collection 6 `interest_sweep_state` and collection 7
//! `trending_refresh_state`.
//!
//! Both tables are written inside plpgsql functions (`sweep_user_topic_interest`,
//! `trending.refresh_agenda_listings`) that cannot reach geo-db, so the job that calls the function
//! reads the row back after the call commits and sends it. Neither collection had a writer, which
//! is why both read 0 rows.
//!
//! Collection 6: group 0, key the row id as text bytes (`incremental`), score `last_run_at` in
//! seconds since the epoch, value `last_run_at` as little-endian i64 microseconds (8 bytes).
//! Collection 7: group 0, key `last_full_pass_on`, score and value the date as days since
//! 1970-01-01 (the value a little-endian i32). A null date writes nothing.
//!
//! The rows are last-write-wins, so the mirror is idempotent and a missed run is repaired by the
//! next. It never fails the job: see `geodb_cursor::derived`.
//!
//! Off unless `GEODB_STATE_MIRROR` is `1` or `true`, on top of `GEODB_DERIVED_MIRROR` and the key
//! wiring, so deploying this code changes nothing in a job that already mirrors other tables. The
//! key's binding must grant `derived=6,7` (`geodb/supervisor.sh`).

use geodb_cursor::derived::{DerivedMirror, DerivedRow};

/// `interest_sweep_state` in geo-db's collection registry.
pub const SWEEP_COLLECTION: u32 = 6;
/// `trending_refresh_state` in geo-db's collection registry.
pub const REFRESH_COLLECTION: u32 = 7;

/// Reads the sweep watermark rows: `(id, last_run_at in microseconds)`.
pub const SWEEP_SQL: &str = "SELECT id, \
    (extract(epoch FROM last_run_at)::numeric * 1000000)::bigint AS last_run_us \
    FROM personalization.interest_sweep_state";

/// Reads the trending watermark: `last_full_pass_on` as days since 1970-01-01, null when unset.
pub const REFRESH_SQL: &str = "SELECT (last_full_pass_on - DATE '1970-01-01')::int AS day \
    FROM trending.refresh_state WHERE id";

/// True when `GEODB_STATE_MIRROR` is `1` or `true`.
pub fn enabled() -> bool {
    enabled_from(std::env::var("GEODB_STATE_MIRROR").ok().as_deref())
}

fn enabled_from(v: Option<&str>) -> bool {
    matches!(v.map(str::trim), Some("1") | Some("true"))
}

/// The mirror, only when both `GEODB_STATE_MIRROR` and the derived mirror are on.
pub fn from_env() -> Option<DerivedMirror> {
    if enabled() {
        DerivedMirror::from_env()
    } else {
        None
    }
}

/// The collection 6 rows for the sweep watermark rows read.
pub fn sweep_rows(rows: &[(String, i64)]) -> Vec<DerivedRow> {
    rows.iter()
        .map(|(id, us)| DerivedRow {
            collection: SWEEP_COLLECTION,
            group: 0,
            score: (*us as f64) / 1_000_000.0,
            key: id.as_bytes().to_vec(),
            value: us.to_le_bytes().to_vec(),
        })
        .collect()
}

/// The collection 7 row for `last_full_pass_on`, none when it is unset.
pub fn refresh_rows(day: Option<i32>) -> Vec<DerivedRow> {
    day.into_iter()
        .map(|d| DerivedRow {
            collection: REFRESH_COLLECTION,
            group: 0,
            score: f64::from(d),
            key: b"last_full_pass_on".to_vec(),
            value: d.to_le_bytes().to_vec(),
        })
        .collect()
}

/// Reads the sweep watermark and sends it. Returns the rows acknowledged. Never fails.
pub async fn mirror_sweep(mirror: &DerivedMirror, pool: &sqlx::PgPool) -> usize {
    match sqlx::query_as::<_, (String, i64)>(SWEEP_SQL)
        .fetch_all(pool)
        .await
    {
        Ok(rows) => {
            mirror
                .put_counted("interest_sweep_state", &sweep_rows(&rows))
                .await
        }
        Err(e) => {
            tracing::warn!(error = %e, "interest sweep state mirror skipped: read failed");
            0
        }
    }
}

/// Reads the trending watermark and sends it. Returns the rows acknowledged. Never fails.
pub async fn mirror_refresh(mirror: &DerivedMirror, pool: &sqlx::PgPool) -> usize {
    match sqlx::query_scalar::<_, Option<i32>>(REFRESH_SQL)
        .fetch_optional(pool)
        .await
    {
        Ok(day) => {
            mirror
                .put_counted("trending_refresh_state", &refresh_rows(day.flatten()))
                .await
        }
        Err(e) => {
            tracing::warn!(error = %e, "trending refresh state mirror skipped: read failed");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_row_is_keyed_by_its_id_with_microseconds() {
        let r = sweep_rows(&[("incremental".into(), 1_700_000_000_500_000)]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].collection, 6);
        assert_eq!(r[0].key, b"incremental");
        assert_eq!(r[0].score, 1_700_000_000.5);
        assert_eq!(r[0].value, 1_700_000_000_500_000i64.to_le_bytes());
        assert!(r[0].put_line().is_ok());
    }

    #[test]
    fn the_refresh_date_is_days_and_a_null_writes_nothing() {
        let r = refresh_rows(Some(20_000));
        assert_eq!(r[0].collection, 7);
        assert_eq!(r[0].score, 20_000.0);
        assert_eq!(r[0].value, 20_000i32.to_le_bytes());
        assert!(r[0].put_line().is_ok());
        assert!(refresh_rows(None).is_empty());
    }

    #[test]
    fn the_flag_is_off_unless_one_or_true() {
        assert!(!enabled_from(None));
        assert!(!enabled_from(Some("0")));
        assert!(!enabled_from(Some("")));
        assert!(enabled_from(Some("1")));
        assert!(enabled_from(Some(" true ")));
    }
}
