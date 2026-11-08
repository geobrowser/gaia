//! Flag-off shadow of the cross-service `meta.block_number` reads against geo-db's `cursors`
//! collection (zero-Postgres plan 10.22 section 6).
//!
//! Two services read another indexer's `meta` row: hermes-pipeline reads the IPFS warmer's block
//! (`hermes_ipfs_cache`) and ranking-indexer reads the pipeline's (`hermes_pipeline`). Each still
//! answers from Postgres. With [`FLAG`] on, a reader also `get`s the writer's row from collection 10
//! (key the writer id, value `<decimal block>:<cursor>`, see [`CursorMirror`](crate::CursorMirror))
//! and counts whether the block numbers agree. The answer never reaches the caller, and a failed
//! read never fails it.
//!
//! The read runs inline after the Postgres read, and at most once per [`MIN_INTERVAL`] per process,
//! so a loop that reads the cursor every few seconds is sampled, not doubled. The writer mirrors
//! after its Postgres commit, so a read can land between the two writes and see geo-db one write
//! behind: judge `mismatch` as a rate and read the logged `pg_block` and `geo_block`, not as a
//! single count.
//!
//! Off unless `GEODB_META_BLOCK_SHADOW` is `1` or `true`. On, it needs `GEODB_KEY_PATH` and
//! `GEODB_BINDING_PATH` like the writer; a read needs only an authenticated session.
//!
//! Metrics: `geodb_meta_block_shadow_total{reader,result}` with `match`, `mismatch` and `failed`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use crate::derived_read::{DerivedReader, Got};

/// The variable that turns the shadow on.
pub const FLAG: &str = "GEODB_META_BLOCK_SHADOW";
pub const TOTAL: &str = "geodb_meta_block_shadow_total";
/// Fewest seconds between two observations in one process.
pub const MIN_INTERVAL: Duration = Duration::from_secs(15);

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

/// The block number a stored value (`<decimal block>:<cursor>`) names; `None` when it is not that shape.
pub fn block_of(value: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(value).ok()?;
    let (block, _cursor) = text.split_once(':')?;
    block.parse().ok()
}

/// Compares the block Postgres answered (`None`: no row, or not a block number) with geo-db's `get`.
pub fn compare(pg: Option<u64>, geo: Option<&Got>) -> (Outcome, Option<u64>) {
    let geo_block = geo.and_then(|g| block_of(&g.value));
    let outcome = match (pg, geo) {
        (None, None) => Outcome::Match,
        (Some(_), None) => Outcome::MissingInGeodb,
        (None, Some(_)) => Outcome::ExtraInGeodb,
        (Some(p), Some(_)) if Some(p) == geo_block => Outcome::Match,
        (Some(_), Some(_)) => Outcome::Differs,
    };
    (outcome, geo_block)
}

pub fn describe() {
    metrics::describe_counter!(
        TOTAL,
        "meta.block_number reads compared with geo-db's cursors collection, by reader and result"
    );
}

pub struct MetaBlockShadow {
    reader: DerivedReader,
    collection: u32,
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl MetaBlockShadow {
    pub fn new(reader: DerivedReader, collection: u32) -> Self {
        Self {
            reader,
            collection,
            min_interval: MIN_INTERVAL,
            last: Mutex::new(None),
        }
    }

    /// Replaces the sampling interval (tests).
    pub fn with_min_interval(mut self, d: Duration) -> Self {
        self.min_interval = d;
        self
    }

    /// `None` when [`FLAG`] is off (the default) or the paths are missing.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let collection = crate::Config::from_vars_for(FLAG, &get)?.collection;
        let reader = DerivedReader::from_vars_for(FLAG, &get)?;
        describe();
        Some(Self::new(reader, collection))
    }

    pub fn from_env() -> Option<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    fn due(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < self.min_interval) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    /// Reads writer `id`'s row and counts it against `pg`, the block the caller's Postgres read
    /// answered. `reader` names the caller in the metric. Skipped inside the sampling interval.
    /// Never fails the caller.
    pub async fn observe(&self, reader: &'static str, id: &str, pg: Option<u64>) {
        if !self.due() {
            return;
        }
        match self
            .reader
            .get_many(self.collection, &[id.as_bytes().to_vec()])
            .await
        {
            Ok(got) => {
                let (outcome, geo_block) = compare(pg, got.first().and_then(|g| g.as_ref()));
                if outcome == Outcome::Match {
                    metrics::counter!(TOTAL, "reader" => reader, "result" => "match").increment(1);
                    info!(
                        event = "geodb_meta_block_shadow_ok",
                        reader, "geo-db meta block matches Postgres"
                    );
                } else {
                    metrics::counter!(TOTAL, "reader" => reader, "result" => "mismatch")
                        .increment(1);
                    warn!(
                        event = "geodb_meta_block_shadow_mismatch",
                        reader,
                        writer = id,
                        kind = outcome.kind(),
                        pg_block = ?pg,
                        geo_block = ?geo_block,
                        "geo-db meta block differs from Postgres"
                    );
                }
            }
            Err(e) => {
                metrics::counter!(TOTAL, "reader" => reader, "result" => "failed").increment(1);
                warn!(event = "geodb_meta_block_shadow.read_failed", reader, error = %e, "shadow read failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn got(value: &str) -> Got {
        Got {
            group: 0,
            score: 0.0,
            value: value.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_block_is_the_text_before_the_first_colon() {
        assert_eq!(block_of(b"72441:v1:abc"), Some(72441));
        assert_eq!(block_of(b"0:"), Some(0));
        for bad in ["", "x", "7", ":cursor", "a:b", "-1:c", " 7:c"] {
            assert_eq!(block_of(bad.as_bytes()), None, "{bad:?}");
        }
        assert_eq!(block_of(&[0xff, b':']), None);
    }

    #[test]
    fn equal_blocks_match_whatever_the_cursor_and_differences_are_named() {
        let g = got("100:opaque");
        assert_eq!(compare(Some(100), Some(&g)), (Outcome::Match, Some(100)));
        assert_eq!(compare(None, None).0, Outcome::Match);
        assert_eq!(compare(Some(99), Some(&g)).0, Outcome::Differs);
        assert_eq!(compare(Some(100), None).0, Outcome::MissingInGeodb);
        assert_eq!(compare(None, Some(&g)).0, Outcome::ExtraInGeodb);
        // A stored value that is not a block never matches a block.
        assert_eq!(compare(Some(1), Some(&got("junk"))).0, Outcome::Differs);
    }

    #[test]
    fn off_by_default_and_needs_paths() {
        assert!(MetaBlockShadow::from_vars(|_| None).is_none());
        assert!(MetaBlockShadow::from_vars(|k| (k == FLAG).then(|| "1".to_string())).is_none());
        let vars = |flag: &'static str| {
            move |k: &str| match k {
                f if f == flag => Some("true".to_string()),
                "GEODB_KEY_PATH" => Some("/k".into()),
                "GEODB_BINDING_PATH" => Some("/b".into()),
                _ => None,
            }
        };
        assert!(MetaBlockShadow::from_vars(vars(FLAG)).is_some());
        assert!(MetaBlockShadow::from_vars(vars("GEODB_CURSOR_MIRROR")).is_none());
    }

    fn fake(script: &str, min_interval: Duration) -> MetaBlockShadow {
        MetaBlockShadow::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            10,
        )
        .with_min_interval(min_interval)
    }

    #[tokio::test]
    async fn an_observation_gets_the_writers_row_and_is_sampled() {
        let dir = std::env::temp_dir().join(format!("meta-shadow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sent");
        let _ = std::fs::remove_file(&log);
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+4\\nnone'; done",
            log.display()
        );
        let shadow = fake(&script, Duration::from_secs(3600));
        shadow.observe("test", "hermes_pipeline", Some(5)).await;
        shadow.observe("test", "hermes_pipeline", Some(6)).await;
        let key_hex: String = "hermes_pipeline"
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        // The second call is inside the interval: one request only.
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("get\t10\thex:{key_hex}\n")
        );
    }

    #[tokio::test]
    async fn a_dead_pipe_is_swallowed() {
        let shadow = fake("exit 1", Duration::ZERO);
        shadow.observe("test", "hermes_pipeline", Some(1)).await;
    }
}
