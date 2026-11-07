//! Flag-off shadow of vote-indexer's reads against geo-db's derived class (zero-Postgres plan, reads
//! twin). Counts only: nothing here changes what the indexer reads, writes or commits.
//!
//! Off unless `VOTE_INDEXER_GEODB_READS_SHADOW` is `1` or `true`. On, it needs `GEODB_KEY_PATH` and
//! `GEODB_BINDING_PATH` like the mirror, and a geodb-client the pod can run (`GEODB_CLIENT_BIN`). A
//! read needs only an authenticated session, not the writer grant. `VOTE_INDEXER_GEODB_READS_SHADOW_SAMPLE`
//! (default 8, at most 64) bounds how many keys one observation reads.
//!
//! # What is twinned
//!
//! * `votes_count` (the vote batch's read, `Storage::get_votes_counts_tx`): the batch's criteria are
//!   read from collection 26 by key and compared with the rows Postgres answered. The mirror writes
//!   after the commit, so a row may or may not have landed when the shadow reads: geo-db matches if
//!   it holds the tally Postgres held before the batch or the one the batch wrote. Runs in the
//!   background after the commit, one observation at a time (an observation is skipped while the last
//!   is running), so it never delays the vote path.
//! * `unscored_entities` (the `new_entity_sweep` page, `Storage::unscored_entities_page`): every entity
//!   Postgres calls unscored must hold no row in the engaged-only overlay (collection 21), because an
//!   overlay row exists only for an engaged entity and an engaged entity has a score row. This checks
//!   the `NOT EXISTS` half of the query.
//!
//! # What geo-db cannot serve today (open gaps)
//!
//! * The sweep's page itself, an `entities` scan by `created_at` in `(created_at, id)` order: the derived
//!   class holds no creation time, and the entity card scan has no created-at order.
//! * `commented_entity_ids` (distinct targets of reply-to relations): needs a relation type scan with a
//!   distinct on the target, which geo-db has no read for.
//! * `get_votes_counts_tx` takes row locks (`FOR UPDATE`) to serialise concurrent batches. geo-db has no
//!   lock; a geo-db serve of this read would have to stay single-writer.
//! * `get_user_votes_tx` and the score recompute's `entities`, `values` and `relations` reads belong to the
//!   keyed user-votes and derived-score twins, not to this one.
//!
//! Metrics: `vote_indexer_geodb_reads_shadow_total{read,result}` (match, mismatch, missing_in_geodb,
//! extra_in_geodb, absent_ok, unsupported) and `vote_indexer_geodb_reads_shadow_failed_total{read,reason}`
//! (read failed, skipped while another observation ran). A mismatch also logs
//! `event=geodb_reads_shadow_mismatch` with counts, never ids.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use geodb_cursor::derived_read::{DerivedReader, Got};
use tracing::{info, warn};
use uuid::Uuid;

use crate::models::{VoteCountCriteria, VotesCountItem};

pub const FLAG: &str = "VOTE_INDEXER_GEODB_READS_SHADOW";
pub const SAMPLE_VAR: &str = "VOTE_INDEXER_GEODB_READS_SHADOW_SAMPLE";
const DEFAULT_SAMPLE: usize = 8;
const MAX_SAMPLE: usize = 64;

pub const TOTAL: &str = "vote_indexer_geodb_reads_shadow_total";
pub const FAILED: &str = "vote_indexer_geodb_reads_shadow_failed_total";

pub const VOTES_COUNT: &str = "votes_count";
pub const UNSCORED_ENTITIES: &str = "unscored_entities";

/// The result of comparing one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verdict {
    Match,
    Mismatch,
    MissingInGeodb,
    ExtraInGeodb,
    /// No row in either store, or only the batch's new row that the mirror has not delivered yet.
    AbsentOk,
    /// geo-db's value is not a layout this twin reads.
    Unsupported,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::Mismatch => "mismatch",
            Verdict::MissingInGeodb => "missing_in_geodb",
            Verdict::ExtraInGeodb => "extra_in_geodb",
            Verdict::AbsentOk => "absent_ok",
            Verdict::Unsupported => "unsupported",
        }
    }
}

/// The `(positive, negative)` a collection 26 value holds: 24 bytes, little-endian `positive`,
/// `negative`, then `updated_at`.
pub fn tally_of(value: &[u8]) -> Option<(i64, i64)> {
    if value.len() != 24 {
        return None;
    }
    Some((
        i64::from_le_bytes(value[0..8].try_into().ok()?),
        i64::from_le_bytes(value[8..16].try_into().ok()?),
    ))
}

/// One `votes_count` key judged. `before` is the row Postgres answered the batch's read with, `after`
/// the row the batch wrote; geo-db must hold one of them.
pub fn judge_votes_count(
    before: Option<(i64, i64)>,
    after: Option<(i64, i64)>,
    geo: Option<&Got>,
) -> Verdict {
    let Some(geo) = geo else {
        return if before.is_some() {
            Verdict::MissingInGeodb
        } else {
            Verdict::AbsentOk
        };
    };
    let Some(held) = tally_of(&geo.value) else {
        return Verdict::Unsupported;
    };
    if before.is_none() && after.is_none() {
        return Verdict::ExtraInGeodb;
    }
    if Some(held) == before || Some(held) == after {
        Verdict::Match
    } else {
        Verdict::Mismatch
    }
}

/// One unscored entity judged: geo-db's engaged-only overlay must hold no row for it.
pub fn judge_unscored(geo: Option<&Got>) -> Verdict {
    if geo.is_some() {
        Verdict::ExtraInGeodb
    } else {
        Verdict::Match
    }
}

pub fn describe() {
    metrics::describe_counter!(
        TOTAL,
        "Sampled vote-indexer reads compared with geo-db's derived class, by read and result"
    );
    metrics::describe_counter!(
        FAILED,
        "Shadow observations that could not run: a read failed, or another observation was still running"
    );
    for read in [VOTES_COUNT, UNSCORED_ENTITIES] {
        for reason in ["read_failed", "busy"] {
            metrics::counter!(FAILED, "read" => read, "reason" => reason).absolute(0);
        }
    }
}

fn record(read: &'static str, verdicts: &[Verdict]) {
    let mut by: HashMap<Verdict, u64> = HashMap::new();
    for v in verdicts {
        *by.entry(*v).or_default() += 1;
    }
    for (v, n) in &by {
        metrics::counter!(TOTAL, "read" => read, "result" => v.label()).increment(*n);
    }
    let bad: u64 = by
        .iter()
        .filter(|(v, _)| {
            matches!(
                v,
                Verdict::Mismatch | Verdict::MissingInGeodb | Verdict::ExtraInGeodb
            )
        })
        .map(|(_, n)| *n)
        .sum();
    if bad > 0 {
        warn!(
            event = "geodb_reads_shadow_mismatch",
            read,
            sampled = verdicts.len(),
            differing = bad,
            "geo-db read twin differs from Postgres for sampled keys"
        );
    } else {
        info!(
            event = "geodb_reads_shadow_ok",
            read,
            sampled = verdicts.len(),
            "geo-db read twin matches Postgres for sampled keys"
        );
    }
}

fn failed(read: &'static str, reason: &'static str) {
    metrics::counter!(FAILED, "read" => read, "reason" => reason).increment(1);
}

pub struct ReadsShadow {
    reader: DerivedReader,
    sample: usize,
    busy: AtomicBool,
}

impl ReadsShadow {
    pub fn new(reader: DerivedReader, sample: usize) -> Self {
        Self {
            reader,
            sample,
            busy: AtomicBool::new(false),
        }
    }

    /// `None` when the flag is off (the default) or misconfigured (logged).
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Arc<Self>> {
        let sample = match get(SAMPLE_VAR).as_deref().map(str::trim) {
            None | Some("") => DEFAULT_SAMPLE,
            Some(s) => match s.parse::<usize>() {
                Ok(n) if (1..=MAX_SAMPLE).contains(&n) => n,
                _ => {
                    warn!(
                        event = "geodb_reads_shadow.misconfigured",
                        "{SAMPLE_VAR} must be 1..={MAX_SAMPLE}; shadow disabled"
                    );
                    return None;
                }
            },
        };
        let reader = DerivedReader::from_vars_for(FLAG, get)?;
        describe();
        Some(Arc::new(Self::new(reader, sample)))
    }

    pub fn from_env() -> Option<Arc<Self>> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Takes the one observation slot; false (counted) while another runs.
    fn claim(&self, read: &'static str) -> bool {
        if self.busy.swap(true, Ordering::AcqRel) {
            failed(read, "busy");
            return false;
        }
        true
    }

    /// Compares the batch's `votes_count` read with collection 26. `criteria` is what the batch asked
    /// for, `before` what Postgres answered, `after` what the batch wrote. Never fails the caller.
    pub async fn observe_votes_count(
        &self,
        criteria: &[VoteCountCriteria],
        before: &[VotesCountItem],
        after: &[VotesCountItem],
    ) {
        if criteria.is_empty() || !self.claim(VOTES_COUNT) {
            return;
        }
        let picked: Vec<_> = criteria.iter().take(self.sample).copied().collect();
        let tally = |rows: &[VotesCountItem]| -> HashMap<VoteCountCriteria, (i64, i64)> {
            rows.iter()
                .map(|c| {
                    (
                        (c.object_id, c.space_id, c.object_type, c.kind),
                        (c.positive, c.negative),
                    )
                })
                .collect()
        };
        let (before, after) = (tally(before), tally(after));
        let keys: Vec<Vec<u8>> = picked
            .iter()
            .map(|(o, s, t, k)| {
                crate::votes_count_mirror::key_of(*o, *s, i16::from(*t), i16::from(*k))
            })
            .collect();
        match self
            .reader
            .get_many(crate::votes_count_mirror::COLLECTION, &keys)
            .await
        {
            Ok(got) => {
                let verdicts: Vec<Verdict> = picked
                    .iter()
                    .zip(got.iter())
                    .map(|(c, g)| {
                        judge_votes_count(before.get(c).copied(), after.get(c).copied(), g.as_ref())
                    })
                    .collect();
                record(VOTES_COUNT, &verdicts);
            }
            Err(e) => {
                warn!(event = "geodb_reads_shadow.read_failed", read = VOTES_COUNT, error = %e, "shadow read failed");
                failed(VOTES_COUNT, "read_failed");
            }
        }
        self.busy.store(false, Ordering::Release);
    }

    /// Checks that none of the entities a sweep page called unscored holds an overlay row.
    pub async fn observe_unscored(&self, entity_ids: &[Uuid]) {
        if entity_ids.is_empty() || !self.claim(UNSCORED_ENTITIES) {
            return;
        }
        let keys: Vec<Vec<u8>> = entity_ids
            .iter()
            .take(self.sample)
            .map(|id| id.as_bytes().to_vec())
            .collect();
        match self
            .reader
            .get_many(crate::entity_scores_mirror::COLLECTION, &keys)
            .await
        {
            Ok(got) => {
                let verdicts: Vec<Verdict> =
                    got.iter().map(|g| judge_unscored(g.as_ref())).collect();
                record(UNSCORED_ENTITIES, &verdicts);
            }
            Err(e) => {
                warn!(event = "geodb_reads_shadow.read_failed", read = UNSCORED_ENTITIES, error = %e, "shadow read failed");
                failed(UNSCORED_ENTITIES, "read_failed");
            }
        }
        self.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ResponseKind, VoteObjectType};
    use geodb_cursor::Config;
    use std::time::Duration;

    fn got(pos: i64, neg: i64) -> Got {
        let mut v = Vec::new();
        v.extend_from_slice(&pos.to_le_bytes());
        v.extend_from_slice(&neg.to_le_bytes());
        v.extend_from_slice(&0i64.to_le_bytes());
        Got {
            group: 0,
            score: 0.0,
            value: v,
        }
    }

    #[test]
    fn a_tally_is_read_from_a_24_byte_value_only() {
        assert_eq!(tally_of(&got(5, 2).value), Some((5, 2)));
        assert_eq!(tally_of(&[0; 16]), None);
        assert_eq!(tally_of(&[]), None);
    }

    #[test]
    fn votes_count_verdicts() {
        let g = got(3, 1);
        // Held before the batch or written by it both match: the mirror may not have landed.
        assert_eq!(
            judge_votes_count(Some((3, 1)), Some((4, 1)), Some(&g)),
            Verdict::Match
        );
        assert_eq!(
            judge_votes_count(Some((2, 1)), Some((3, 1)), Some(&g)),
            Verdict::Match
        );
        assert_eq!(
            judge_votes_count(None, Some((3, 1)), Some(&g)),
            Verdict::Match
        );
        assert_eq!(
            judge_votes_count(Some((9, 9)), Some((8, 8)), Some(&g)),
            Verdict::Mismatch
        );
        // Postgres had a row before the batch and geo-db has none.
        assert_eq!(
            judge_votes_count(Some((3, 1)), Some((4, 1)), None),
            Verdict::MissingInGeodb
        );
        // A brand new row the mirror has not delivered yet, or nothing anywhere.
        assert_eq!(
            judge_votes_count(None, Some((1, 0)), None),
            Verdict::AbsentOk
        );
        assert_eq!(judge_votes_count(None, None, None), Verdict::AbsentOk);
        assert_eq!(
            judge_votes_count(None, None, Some(&g)),
            Verdict::ExtraInGeodb
        );
        let odd = Got {
            value: vec![1, 2, 3],
            ..g
        };
        assert_eq!(
            judge_votes_count(Some((3, 1)), None, Some(&odd)),
            Verdict::Unsupported
        );
    }

    #[test]
    fn an_unscored_entity_must_hold_no_overlay_row() {
        assert_eq!(judge_unscored(None), Verdict::Match);
        assert_eq!(judge_unscored(Some(&got(0, 0))), Verdict::ExtraInGeodb);
    }

    #[test]
    fn off_by_default_and_a_bad_sample_disables_it() {
        let none = |_: &str| None;
        assert!(ReadsShadow::from_vars(none).is_none());
        let on = |sample: Option<&'static str>| {
            move |k: &str| match k {
                FLAG => Some("1".to_string()),
                "GEODB_KEY_PATH" => Some("/k".into()),
                "GEODB_BINDING_PATH" => Some("/b".into()),
                SAMPLE_VAR => sample.map(str::to_string),
                _ => None,
            }
        };
        assert!(ReadsShadow::from_vars(on(None)).is_some());
        assert!(ReadsShadow::from_vars(on(Some("64"))).is_some());
        for bad in ["0", "65", "x", "-1"] {
            assert!(ReadsShadow::from_vars(on(Some(bad))).is_none(), "{bad}");
        }
        // Flagged but no key or binding path: off.
        let no_paths = |k: &str| (k == FLAG).then(|| "1".to_string());
        assert!(ReadsShadow::from_vars(no_paths).is_none());
    }

    fn fake(script: &str, sample: usize) -> ReadsShadow {
        ReadsShadow::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            sample,
        )
    }

    fn item(n: u8, pos: i64, neg: i64) -> VotesCountItem {
        VotesCountItem {
            object_id: Uuid::from_bytes([n; 16]),
            object_type: VoteObjectType::Entity,
            space_id: Uuid::from_bytes([9; 16]),
            kind: ResponseKind::Curation,
            positive: pos,
            negative: neg,
        }
    }

    fn crit(i: &VotesCountItem) -> VoteCountCriteria {
        (i.object_id, i.space_id, i.object_type, i.kind)
    }

    fn script_in(dir: &str, answer: &str) -> (String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vote-shadow-{dir}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sent");
        let _ = std::fs::remove_file(&log);
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '{answer}'; done",
            log.display()
        );
        (script, log)
    }

    #[tokio::test]
    async fn votes_count_observation_reads_only_the_sample_and_frees_the_slot() {
        let (script, log) = script_in("vc", "+4\\nnone");
        let shadow = fake(&script, 2);
        let (a, b, c) = (item(1, 3, 1), item(2, 1, 0), item(3, 1, 0));
        shadow
            .observe_votes_count(
                &[crit(&a), crit(&b), crit(&c)],
                std::slice::from_ref(&a),
                &[a.clone(), b.clone(), c.clone()],
            )
            .await;
        let sent = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<_> = sent.lines().collect();
        assert_eq!(lines.len(), 2, "{sent}");
        assert!(lines.iter().all(|l| l.starts_with("get\t26\thex:")));
        assert!(!shadow.busy.load(Ordering::Acquire), "slot freed");
        // A second observation runs: the slot was released.
        shadow
            .observe_votes_count(&[crit(&c)], &[], std::slice::from_ref(&c))
            .await;
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 3);
    }

    #[tokio::test]
    async fn unscored_observation_reads_collection_21_and_a_busy_slot_skips() {
        let (script, log) = script_in("un", "+4\\nnone");
        let shadow = fake(&script, 8);
        let ids = [Uuid::from_bytes([1; 16]), Uuid::from_bytes([2; 16])];
        shadow.observe_unscored(&ids).await;
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(sent.lines().count(), 2);
        assert!(sent.lines().all(|l| l.starts_with("get\t21\thex:")));
        // While another observation holds the slot, this one does nothing.
        shadow.busy.store(true, Ordering::Release);
        shadow.observe_unscored(&ids).await;
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_dead_pipe_is_swallowed_and_frees_the_slot() {
        let shadow = fake("exit 1", 8);
        let a = item(1, 1, 0);
        shadow
            .observe_votes_count(&[crit(&a)], &[], std::slice::from_ref(&a))
            .await;
        assert!(!shadow.busy.load(Ordering::Acquire));
    }
}
