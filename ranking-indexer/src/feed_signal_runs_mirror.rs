//! Mirrors the newest `personalization.feed_signal_runs` row of each status into geo-db's
//! `cursors` collection (collection 10), beside the chain and poll cursors.
//!
//! The run log is written inside plpgsql (`replace_feed_signals` for a success,
//! `record_feed_signal_run` for the other two) that cannot reach geo-db, and its readers do not
//! want the log: `feed_signal_window` and the request path `readSeen` want the newest succeeded
//! `data_through`, and `feed_signal_freshness` wants the newest `finished_at` of a succeeded or
//! unchanged run plus that same `data_through`. Three keys carry all of it, one per status, so the
//! history (retention `run_retention_days`) stays a Postgres concern and can become a metric.
//!
//! Key `feed_signal_run:<status>` (status is `succeeded`, `unchanged` or `failed`); the value is the
//! `cursors` shape `<block>:<cursor>` that `geodb_cursor::CursorMirror::mirror` writes, where
//! `block` is `finished_at` in microseconds since the epoch and `cursor` is
//! `<data_through as RFC 3339 with microseconds, Z, or "-" when null>/<generation, empty when null>`.
//! The error text is not mirrored. Rows are last-write-wins, so a missed run is repaired by the next.
//!
//! Off unless `GEODB_FEED_RUNS_MIRROR` is `1` or `true`. The scores key's binding does not grant
//! collection 10, so this mirror takes its own key path from `GEODB_FEED_RUNS_KEY_PATH` (the
//! `geodb-cursor-key` principal, `derived=10`), with the client binary and binding shared with the
//! derived mirror. It never fails the job.

use chrono::{DateTime, SecondsFormat, Utc};
use geodb_cursor::{Config, CursorMirror};

/// Prefix of the `cursors` keys, apart from chain cursors and `notification_poll:`.
pub const KEY_PREFIX: &str = "feed_signal_run:";

/// The newest run of each status.
pub const SELECT_SQL: &str =
    "SELECT DISTINCT ON (status) status, finished_at, generation, data_through \
    FROM personalization.feed_signal_runs ORDER BY status, finished_at DESC, id DESC";

/// One run, as the mirror reads it back.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Run {
    pub status: String,
    pub finished_at: DateTime<Utc>,
    pub generation: Option<String>,
    pub data_through: Option<DateTime<Utc>>,
}

/// The `cursors` key of a status.
pub fn mirror_key(status: &str) -> String {
    format!("{KEY_PREFIX}{status}")
}

/// The `(cursor, block)` arguments of `CursorMirror::mirror`.
pub fn mirror_parts(r: &Run) -> (String, u64) {
    let through = r.data_through.map_or_else(
        || "-".to_string(),
        |t| t.to_rfc3339_opts(SecondsFormat::Micros, true),
    );
    (
        format!("{through}/{}", r.generation.as_deref().unwrap_or("")),
        r.finished_at.timestamp_micros().max(0) as u64,
    )
}

/// Reads a stored value back to `(finished_at, data_through, generation)`; `None` when it is not
/// that shape. A shadow read compares with this.
#[allow(clippy::type_complexity)]
pub fn parse_mirror_value(
    value: &str,
) -> Option<(DateTime<Utc>, Option<DateTime<Utc>>, Option<String>)> {
    let (block, cursor) = value.split_once(':')?;
    let finished = DateTime::from_timestamp_micros(block.parse::<i64>().ok()?)?;
    let (through, generation) = cursor.split_once('/')?;
    let through = if through == "-" {
        None
    } else {
        Some(
            DateTime::parse_from_rfc3339(through)
                .ok()?
                .with_timezone(&Utc),
        )
    };
    let generation = (!generation.is_empty()).then(|| generation.to_string());
    Some((finished, through, generation))
}

/// The mirror, only when `GEODB_FEED_RUNS_MIRROR` is on and wired.
pub fn from_env() -> Option<CursorMirror> {
    from_vars(|k| std::env::var(k).ok()).map(CursorMirror::new)
}

fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Config> {
    Config::from_vars_for("GEODB_FEED_RUNS_MIRROR", |k| match k {
        "GEODB_KEY_PATH" => get("GEODB_FEED_RUNS_KEY_PATH"),
        other => get(other),
    })
}

/// Reads the newest run of each status and sends it. Returns the rows sent. Never fails.
pub async fn mirror_latest(mirror: &CursorMirror, pool: &sqlx::PgPool) -> usize {
    match sqlx::query_as::<_, Run>(SELECT_SQL).fetch_all(pool).await {
        Ok(runs) => {
            for r in &runs {
                let (cursor, block) = mirror_parts(r);
                mirror.mirror(&mirror_key(&r.status), &cursor, block).await;
            }
            runs.len()
        }
        Err(e) => {
            tracing::warn!(error = %e, "feed signal runs mirror skipped: read failed");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn run(status: &str, through: Option<&str>, generation: Option<&str>) -> Run {
        Run {
            status: status.into(),
            finished_at: ts("2026-11-06T12:34:56.123456Z"),
            generation: generation.map(Into::into),
            data_through: through.map(ts),
        }
    }

    #[test]
    fn keys_are_prefixed_per_status() {
        assert_eq!(mirror_key("succeeded"), "feed_signal_run:succeeded");
        assert_eq!(mirror_key("unchanged"), "feed_signal_run:unchanged");
        assert_eq!(mirror_key("failed"), "feed_signal_run:failed");
    }

    #[test]
    fn the_value_is_microseconds_then_watermark_and_generation() {
        let (cursor, block) =
            mirror_parts(&run("succeeded", Some("2026-11-06T12:00:00Z"), Some("g2")));
        assert_eq!(block, 1_793_968_496_123_456);
        assert_eq!(cursor, "2026-11-06T12:00:00.000000Z/g2");
        // `CursorMirror::mirror` joins them as "{block}:{cursor}".
        assert_eq!(
            format!("{block}:{cursor}"),
            "1793968496123456:2026-11-06T12:00:00.000000Z/g2"
        );
    }

    #[test]
    fn a_failed_run_with_no_cutoff_has_a_dash_and_no_generation() {
        let (cursor, _) = mirror_parts(&run("failed", None, None));
        assert_eq!(cursor, "-/");
    }

    #[test]
    fn a_value_fits_the_cursors_bound() {
        let g = "g".repeat(200);
        let (cursor, block) =
            mirror_parts(&run("succeeded", Some("2026-11-06T12:00:00Z"), Some(&g)));
        assert!(format!("{block}:{cursor}").len() <= geodb_cursor::VALUE_MAX);
    }

    #[test]
    fn values_round_trip_including_a_slash_in_the_generation() {
        for r in [
            run("succeeded", Some("2026-11-06T12:00:00.5Z"), Some("a/b:c")),
            run("unchanged", Some("2026-11-06T12:00:00Z"), Some("g")),
            run("failed", None, None),
        ] {
            let (cursor, block) = mirror_parts(&r);
            assert_eq!(
                parse_mirror_value(&format!("{block}:{cursor}")),
                Some((r.finished_at, r.data_through, r.generation.clone()))
            );
        }
    }

    #[test]
    fn a_malformed_value_does_not_parse() {
        for v in [
            "",
            "x",
            "1",
            "1:-",
            "a:-/",
            "1:yesterday/g",
            "1:2026-01-01/g",
        ] {
            assert_eq!(parse_mirror_value(v), None, "{v:?}");
        }
    }

    #[test]
    fn the_select_takes_the_newest_row_of_each_status() {
        assert!(SELECT_SQL.contains("DISTINCT ON (status)"));
        assert!(SELECT_SQL.contains("ORDER BY status, finished_at DESC, id DESC"));
    }

    fn vars(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let m: std::collections::HashMap<_, _> = pairs.iter().copied().collect();
        move |k| m.get(k).map(|v| v.to_string())
    }

    #[test]
    fn the_flag_is_off_unless_one_or_true() {
        let base = [
            ("GEODB_FEED_RUNS_KEY_PATH", "/k"),
            ("GEODB_BINDING_PATH", "/b"),
        ];
        let with = |flag: &'static str| {
            let mut v = base.to_vec();
            v.push(("GEODB_FEED_RUNS_MIRROR", flag));
            from_vars(vars(&v))
        };
        assert!(from_vars(vars(&base)).is_none());
        assert!(with("0").is_none());
        assert!(with("").is_none());
        assert!(with("1").is_some());
        assert!(with("true").is_some());
    }

    #[test]
    fn it_uses_its_own_key_not_the_scores_key() {
        let cfg = from_vars(vars(&[
            ("GEODB_FEED_RUNS_MIRROR", "1"),
            ("GEODB_FEED_RUNS_KEY_PATH", "/cursor.key"),
            ("GEODB_KEY_PATH", "/scores.key"),
            ("GEODB_BINDING_PATH", "/b"),
        ]))
        .unwrap();
        let args: Vec<_> = cfg
            .args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"/cursor.key".to_string()));
        assert!(!args.contains(&"/scores.key".to_string()));
        assert_eq!(cfg.collection, 10);
    }

    #[test]
    fn no_key_path_means_off() {
        assert!(from_vars(vars(&[
            ("GEODB_FEED_RUNS_MIRROR", "1"),
            ("GEODB_KEY_PATH", "/scores.key"),
            ("GEODB_BINDING_PATH", "/b"),
        ]))
        .is_none());
    }
}
