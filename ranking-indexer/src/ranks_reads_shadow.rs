//! Flag-off shadow of ranking-indexer's `ranks.ranking_scores` reads against geo-db's derived class
//! (zero-Postgres plan, reads twin). Counts only: it never changes what the indexer reads, writes
//! or commits.
//!
//! Off unless `RANKING_INDEXER_GEODB_READS_SHADOW` is `1` or `true`. On, it needs `GEODB_KEY_PATH`
//! and `GEODB_BINDING_PATH` like the mirror (`GEODB_CLIENT_BIN` optional). A read needs only an
//! authenticated session, not the writer grant.
//!
//! # What is twinned
//!
//! `ranks.ranking_scores`, read by block (collection 11, group = the block id): after a block's
//! recompute commits and the mirror has sent it, one background task scans the block's group and
//! compares it with the rows Postgres now holds (the rows the recompute inserted). A block of up to
//! [`SCAN_MAX`] rows is compared whole (same keys, scores and positions); a larger block is compared
//! as a subset, because one scan page is all geo-db returns. One observation runs at a time per
//! process, so a backfill samples blocks rather than queueing behind itself.
//!
//! Every downstream reader of `ranks.ranking_scores` (the agenda, debate-watch, topic and entity
//! score computes) reads by block, so this scan is the read they would be served from.
//!
//! # What geo-db cannot serve today (open gaps)
//!
//! * `ranks.rankings`, `ranks.ranking_blocks` and `ranks.ranking_items` are keyed records in the votes
//!   system Space, with BLAKE3 record keys. Reading them needs the card reader plus a Space map, which
//!   the Rust side does not have. They are the TS `keyedRecordsShadow` twin's.
//! * `ranks.members` and `ranks.editors`: membership is a Space-level relation, not a derived
//!   collection.
//! * The joins (`ranking_blocks` with `rankings` for the block's rankers, the split-edit anti-joins on
//!   `from_entity_id NOT IN (SELECT id ...)`) have no geo-db read.
//! * Postgres orders by `score DESC`; the scan's order is geo-db's, so order is not compared, only the
//!   position stored in each row.
//!
//! Metrics: `ranking_indexer_geodb_reads_shadow_total{read,result}` (match, mismatch,
//! missing_in_geodb, extra_in_geodb) counts keys, and
//! `ranking_indexer_geodb_reads_shadow_failed_total{read,reason}` (read_failed, busy) counts
//! observations that did not run. A difference also logs `event=geodb_reads_shadow_mismatch` with
//! counts, never ids.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use geodb_cursor::derived_read::{DerivedReader, ScanRow, SCAN_MAX};
use tracing::{info, warn};
use uuid::Uuid;

use crate::derived_mirror::COLLECTION;
use crate::scoring::ScoreRow;

pub const FLAG: &str = "RANKING_INDEXER_GEODB_READS_SHADOW";
pub const TOTAL: &str = "ranking_indexer_geodb_reads_shadow_total";
pub const FAILED: &str = "ranking_indexer_geodb_reads_shadow_failed_total";
pub const READ: &str = "ranking_scores_by_block";

/// What one block's comparison found, in keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub matched: u64,
    pub mismatched: u64,
    pub missing_in_geodb: u64,
    pub extra_in_geodb: u64,
}

impl Counts {
    pub fn differing(&self) -> u64 {
        self.mismatched + self.missing_in_geodb + self.extra_in_geodb
    }
}

fn key(block: Uuid, entity: Uuid, space: Uuid) -> Vec<u8> {
    let mut k = Vec::with_capacity(48);
    k.extend_from_slice(block.as_bytes());
    k.extend_from_slice(entity.as_bytes());
    k.extend_from_slice(space.as_bytes());
    k
}

fn position_of(value: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(value.try_into().ok()?))
}

/// Compares the rows Postgres holds for `block` with the page geo-db returned. `more` is whether
/// geo-db has rows beyond the page; then only a subset check is possible.
pub fn compare(block: Uuid, pg: &[ScoreRow], geo: &[ScanRow], more: bool) -> Counts {
    let want: HashMap<Vec<u8>, &ScoreRow> = pg
        .iter()
        .map(|r| (key(block, r.entity_id, r.space_id), r))
        .collect();
    let mut c = Counts::default();
    let mut seen = std::collections::HashSet::with_capacity(geo.len());
    for g in geo {
        seen.insert(g.key.as_slice());
        match want.get(&g.key) {
            None => c.extra_in_geodb += 1,
            Some(r) => {
                let same = position_of(&g.value) == Some(r.position.max(0) as u32)
                    && g.score.to_bits() == r.score.to_bits();
                if same {
                    c.matched += 1;
                } else {
                    c.mismatched += 1;
                }
            }
        }
    }
    if !more {
        c.missing_in_geodb = want.keys().filter(|k| !seen.contains(k.as_slice())).count() as u64;
    }
    c
}

pub fn describe() {
    metrics::describe_counter!(
        TOTAL,
        "ranks.ranking_scores keys compared with geo-db's derived class, by result"
    );
    metrics::describe_counter!(
        FAILED,
        "Shadow observations that did not run: a read failed, or another was still running"
    );
    for reason in ["read_failed", "busy"] {
        metrics::counter!(FAILED, "read" => READ, "reason" => reason).absolute(0);
    }
    for result in ["match", "mismatch", "missing_in_geodb", "extra_in_geodb"] {
        metrics::counter!(TOTAL, "read" => READ, "result" => result).absolute(0);
    }
}

fn record(c: Counts) {
    for (result, n) in [
        ("match", c.matched),
        ("mismatch", c.mismatched),
        ("missing_in_geodb", c.missing_in_geodb),
        ("extra_in_geodb", c.extra_in_geodb),
    ] {
        metrics::counter!(TOTAL, "read" => READ, "result" => result).increment(n);
    }
    if c.differing() > 0 {
        warn!(
            event = "geodb_reads_shadow_mismatch",
            read = READ,
            matched = c.matched,
            mismatched = c.mismatched,
            missing_in_geodb = c.missing_in_geodb,
            extra_in_geodb = c.extra_in_geodb,
            "geo-db read twin differs from Postgres for a block"
        );
    } else {
        info!(
            event = "geodb_reads_shadow_ok",
            read = READ,
            keys = c.matched,
            "geo-db read twin matches Postgres"
        );
    }
}

pub struct RanksReadsShadow {
    reader: DerivedReader,
    busy: AtomicBool,
}

impl RanksReadsShadow {
    pub fn new(reader: DerivedReader) -> Self {
        Self {
            reader,
            busy: AtomicBool::new(false),
        }
    }

    /// `None` when the flag is off (the default) or the paths are missing.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Arc<Self>> {
        let reader = DerivedReader::from_vars_for(FLAG, get)?;
        describe();
        Some(Arc::new(Self::new(reader)))
    }

    pub fn from_env() -> Option<Arc<Self>> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Compares `block`'s stored rows with geo-db. Skipped (counted) while another observation
    /// runs. Never fails the caller.
    pub async fn observe_block(&self, block: Uuid, pg: &[ScoreRow]) {
        if self.busy.swap(true, Ordering::AcqRel) {
            metrics::counter!(FAILED, "read" => READ, "reason" => "busy").increment(1);
            return;
        }
        let group = u128::from_be_bytes(*block.as_bytes());
        match self.reader.scan(COLLECTION, group, SCAN_MAX).await {
            Ok(page) => record(compare(block, pg, &page.rows, page.more)),
            Err(e) => {
                warn!(event = "geodb_reads_shadow.read_failed", read = READ, error = %e, "shadow read failed");
                metrics::counter!(FAILED, "read" => READ, "reason" => "read_failed").increment(1);
            }
        }
        self.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geodb_cursor::Config;
    use std::time::Duration;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn pg(e: u8, score: f64, position: i32) -> ScoreRow {
        ScoreRow {
            entity_id: u(e),
            space_id: u(9),
            score,
            position,
        }
    }

    fn geo(block: u8, e: u8, score: f64, position: u32) -> ScanRow {
        ScanRow {
            score,
            key: key(u(block), u(e), u(9)),
            value: position.to_le_bytes().to_vec(),
        }
    }

    #[test]
    fn identical_blocks_match_in_any_order() {
        let rows = [pg(1, 2.5, 1), pg(2, 1.0, 2)];
        let g = [geo(7, 2, 1.0, 2), geo(7, 1, 2.5, 1)];
        let c = compare(u(7), &rows, &g, false);
        assert_eq!(
            c,
            Counts {
                matched: 2,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn differences_are_counted_by_kind() {
        let rows = [pg(1, 2.5, 1), pg(2, 1.0, 2), pg(3, 0.5, 3)];
        // 1 differs in position, 2 is absent from geo-db, 4 is only in geo-db.
        let g = [geo(7, 1, 2.5, 5), geo(7, 3, 0.5, 3), geo(7, 4, 0.1, 4)];
        let c = compare(u(7), &rows, &g, false);
        assert_eq!(
            c,
            Counts {
                matched: 1,
                mismatched: 1,
                missing_in_geodb: 1,
                extra_in_geodb: 1
            }
        );
        // A different score is a mismatch too.
        let c = compare(u(7), &[pg(1, 2.5, 1)], &[geo(7, 1, 2.6, 1)], false);
        assert_eq!(c.mismatched, 1);
    }

    #[test]
    fn a_truncated_page_is_a_subset_check() {
        let rows = [pg(1, 2.5, 1), pg(2, 1.0, 2)];
        let c = compare(u(7), &rows, &[geo(7, 1, 2.5, 1)], true);
        assert_eq!(
            c,
            Counts {
                matched: 1,
                ..Counts::default()
            }
        );
        // A malformed position is a mismatch, never a panic.
        let bad = ScanRow {
            value: vec![1],
            ..geo(7, 1, 2.5, 1)
        };
        assert_eq!(compare(u(7), &rows, &[bad], true).mismatched, 1);
    }

    #[test]
    fn off_by_default_and_needs_paths() {
        assert!(RanksReadsShadow::from_vars(|_| None).is_none());
        assert!(RanksReadsShadow::from_vars(|k| (k == FLAG).then(|| "1".to_string())).is_none());
        let on = |k: &str| match k {
            FLAG => Some("true".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        };
        assert!(RanksReadsShadow::from_vars(on).is_some());
        let off = |k: &str| match k {
            FLAG => Some("0".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        };
        assert!(RanksReadsShadow::from_vars(off).is_none());
    }

    fn fake(script: &str) -> RanksReadsShadow {
        RanksReadsShadow::new(DerivedReader::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            collection: 0,
            timeout: Duration::from_millis(1500),
            cooldown: Duration::from_secs(60),
        }))
    }

    #[tokio::test]
    async fn an_observation_scans_the_blocks_group_and_frees_the_slot() {
        let dir = std::env::temp_dir().join(format!("ranks-shadow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sent");
        let _ = std::fs::remove_file(&log);
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+0\\n'; done",
            log.display()
        );
        let shadow = fake(&script);
        shadow.observe_block(u(7), &[pg(1, 1.0, 1)]).await;
        let sent = std::fs::read_to_string(&log).unwrap();
        let group = u128::from_be_bytes(*u(7).as_bytes());
        assert_eq!(
            sent.trim_end(),
            format!("scan\t11\t0x{group:x}\t{SCAN_MAX}\t1")
        );
        assert!(!shadow.busy.load(Ordering::Acquire));
        // A held slot skips the observation without reading.
        shadow.busy.store(true, Ordering::Release);
        shadow.observe_block(u(7), &[]).await;
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    }

    #[tokio::test]
    async fn a_dead_pipe_is_swallowed_and_frees_the_slot() {
        let shadow = fake("exit 1");
        shadow.observe_block(u(7), &[pg(1, 1.0, 1)]).await;
        assert!(!shadow.busy.load(Ordering::Acquire));
    }
}
