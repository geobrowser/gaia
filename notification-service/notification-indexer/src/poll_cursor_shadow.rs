//! Flag-off shadow read of `notification_poll_cursors` against geo-db's `cursors` collection.
//!
//! Postgres stays the source: `Storage::get_poll_cursor` answers from Postgres exactly as before, and
//! with the flag on it also `get`s `notification_poll:<name>` from collection 10 and counts whether the
//! two agree. The result never reaches the caller and a failed read never fails the poller.
//!
//! Off unless `NOTIFICATION_INDEXER_GEODB_POLL_CURSOR_SHADOW` is `1` or `true`. On, it needs
//! `GEODB_KEY_PATH` and `GEODB_BINDING_PATH` like the writer (`GEODB_CLIENT_BIN` optional). A read needs
//! only an authenticated session, not the writer grant, and the reader holds its own pipe child.
//!
//! The read runs inline after the Postgres read, not in a background task: the poller writes the cursor
//! (and mirrors it) later in the same tick, so a deferred read could see the new value against the old
//! Postgres answer and count a false mismatch. The reader bounds each call (2 s) and cools down after a
//! failure, so a sick geo-db costs the poller at most one bounded wait per cool-down.
//!
//! Comparison is on bytes: the value geo-db holds must equal `<id>:<timestamp>` as the writer builds it
//! from the Postgres row (`poll_cursor_mirror::mirror_parts`). A cursor Postgres has never had and geo-db
//! does not hold is a match.
//!
//! Metrics: `notification_indexer_geodb_poll_cursor_shadow_total{result}` with `match`, `mismatch` and
//! `failed`. A mismatch also logs `event=geodb_poll_cursor_shadow_mismatch` with the kind (`differs`,
//! `missing_in_geodb`, `extra_in_geodb`) and the cursor name (a fixed set of poller names, no ids).

use chrono::{DateTime, Utc};
use geodb_cursor::derived_read::{DerivedReader, Got};
use tracing::{info, warn};

use crate::poll_cursor_mirror::{mirror_key, mirror_parts};

pub const FLAG: &str = "NOTIFICATION_INDEXER_GEODB_POLL_CURSOR_SHADOW";
pub const TOTAL: &str = "notification_indexer_geodb_poll_cursor_shadow_total";

/// What one comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Match,
    Differs,
    MissingInGeodb,
    ExtraInGeodb,
}

impl Outcome {
    fn kind(self) -> &'static str {
        match self {
            Outcome::Match => "match",
            Outcome::Differs => "differs",
            Outcome::MissingInGeodb => "missing_in_geodb",
            Outcome::ExtraInGeodb => "extra_in_geodb",
        }
    }
}

/// Compares the Postgres row `pg` (`cursor_updated_at`, `cursor_id`) with geo-db's `get` answer.
pub fn compare(pg: Option<(DateTime<Utc>, i64)>, geo: Option<&Got>) -> Outcome {
    match (pg, geo) {
        (None, None) => Outcome::Match,
        (Some(_), None) => Outcome::MissingInGeodb,
        (None, Some(_)) => Outcome::ExtraInGeodb,
        (Some((ts, id)), Some(g)) => {
            let (cursor, block) = mirror_parts(ts, id);
            if g.value == format!("{block}:{cursor}").into_bytes() {
                Outcome::Match
            } else {
                Outcome::Differs
            }
        }
    }
}

pub fn describe() {
    metrics::describe_counter!(
        TOTAL,
        "notification_poll_cursors reads compared with geo-db's cursors collection, by result"
    );
    for result in ["match", "mismatch", "failed"] {
        metrics::counter!(TOTAL, "result" => result).absolute(0);
    }
}

pub struct PollCursorShadow {
    reader: DerivedReader,
    collection: u32,
}

impl PollCursorShadow {
    pub fn new(reader: DerivedReader, collection: u32) -> Self {
        Self { reader, collection }
    }

    /// `None` when the flag is off (the default) or the paths are missing.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let collection = geodb_cursor::Config::from_vars_for(FLAG, &get)?.collection;
        let reader = DerivedReader::from_vars_for(FLAG, &get)?;
        describe();
        Some(Self::new(reader, collection))
    }

    pub fn from_env() -> Option<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Reads `name` from geo-db and counts it against what Postgres answered. Never fails the caller.
    pub async fn observe(&self, name: &str, pg: Option<(DateTime<Utc>, i64)>) {
        let key = mirror_key(name).into_bytes();
        match self.reader.get_many(self.collection, &[key]).await {
            Ok(got) => {
                let outcome = compare(pg, got.first().and_then(|g| g.as_ref()));
                if outcome == Outcome::Match {
                    metrics::counter!(TOTAL, "result" => "match").increment(1);
                    info!(
                        event = "geodb_poll_cursor_shadow_ok",
                        cursor = name,
                        "geo-db poll cursor matches Postgres"
                    );
                } else {
                    metrics::counter!(TOTAL, "result" => "mismatch").increment(1);
                    warn!(
                        event = "geodb_poll_cursor_shadow_mismatch",
                        cursor = name,
                        kind = outcome.kind(),
                        "geo-db poll cursor differs from Postgres"
                    );
                }
            }
            Err(e) => {
                metrics::counter!(TOTAL, "result" => "failed").increment(1);
                warn!(event = "geodb_poll_cursor_shadow.read_failed", cursor = name, error = %e, "shadow read failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geodb_cursor::Config;
    use std::time::Duration;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn got(value: &str) -> Got {
        Got {
            group: 0,
            score: 0.0,
            value: value.as_bytes().to_vec(),
        }
    }

    #[test]
    fn equal_rows_match_and_differences_are_named() {
        let pg = Some((ts("2026-11-06T12:34:56.123456Z"), 42));
        let g = got("42:2026-11-06T12:34:56.123456Z");
        assert_eq!(compare(pg, Some(&g)), Outcome::Match);
        assert_eq!(compare(None, None), Outcome::Match);
        assert_eq!(
            compare(pg, Some(&got("41:2026-11-06T12:34:56.123456Z"))),
            Outcome::Differs
        );
        assert_eq!(
            compare(pg, Some(&got("42:2026-11-06T12:34:56.123457Z"))),
            Outcome::Differs
        );
        assert_eq!(compare(pg, None), Outcome::MissingInGeodb);
        assert_eq!(compare(None, Some(&g)), Outcome::ExtraInGeodb);
    }

    #[test]
    fn a_negative_id_is_compared_as_the_writer_sends_it() {
        let pg = Some((ts("2026-01-01T00:00:00Z"), -3));
        assert_eq!(
            compare(pg, Some(&got("0:2026-01-01T00:00:00.000000Z"))),
            Outcome::Match
        );
    }

    #[test]
    fn off_by_default_and_needs_paths() {
        assert!(PollCursorShadow::from_vars(|_| None).is_none());
        assert!(PollCursorShadow::from_vars(|k| (k == FLAG).then(|| "1".to_string())).is_none());
        let vars = |flag: &'static str| {
            move |k: &str| match k {
                f if f == flag => Some("1".to_string()),
                "GEODB_KEY_PATH" => Some("/k".into()),
                "GEODB_BINDING_PATH" => Some("/b".into()),
                _ => None,
            }
        };
        assert!(PollCursorShadow::from_vars(vars(FLAG)).is_some());
        // The writer's flag does not turn the shadow on.
        assert!(PollCursorShadow::from_vars(vars("GEODB_CURSOR_MIRROR")).is_none());
    }

    fn fake(script: &str) -> PollCursorShadow {
        PollCursorShadow::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            10,
        )
    }

    #[tokio::test]
    async fn an_observation_gets_the_prefixed_key_from_the_collection() {
        let dir = std::env::temp_dir().join(format!("poll-shadow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sent");
        let _ = std::fs::remove_file(&log);
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+4\\nnone'; done",
            log.display()
        );
        let shadow = fake(&script);
        shadow.observe("entity_votes_threshold", None).await;
        let key_hex: String = "notification_poll:entity_votes_threshold"
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().trim_end(),
            format!("get\t10\thex:{key_hex}")
        );
    }

    #[tokio::test]
    async fn a_dead_pipe_is_swallowed() {
        let shadow = fake("exit 1");
        shadow
            .observe(
                "entity_votes_threshold",
                Some((ts("2026-01-01T00:00:00Z"), 1)),
            )
            .await;
    }
}
