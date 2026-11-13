//! The vote poller's "changed since" read answered from geo-db's `votes_count_updates` (derived
//! collection 33, plan 10.22 section 29.6), behind a flag that defaults off.
//!
//! `Storage::find_entity_vote_counts_since` reads `votes_count` rows with `object_type = 0 AND
//! vote_kind = 0` in `(updated_at, id)` order. Collection 33 holds one row for each row of
//! `votes_count`, written by the vote indexer beside its row of collection 26: group `object_type
//! << 8 | vote_kind`, so the poller's predicate is group 0; key the 34 bytes of 26 (object id, Space
//! id, type, kind); score minus the update time in microseconds, so a scan walks the oldest update
//! first and equal times by key; value the 24 bytes of 26 (`positive`, `negative`, `updated_us`,
//! little-endian `i64`).
//!
//! `NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES` (needs `GEODB_KEY_PATH` and `GEODB_BINDING_PATH`, like
//! the other geo-db readers; `GEODB_CLIENT_BIN` optional):
//! - unset, or anything else: off. Nothing here runs.
//! - `shadow`: Postgres answers the poller exactly as before. After the first page of a tick, the
//!   same page is read from collection 33 and compared; the result is a count
//!   ([`TOTAL`]: `match`, `mismatch`, `failed`) and a log line carrying counts only. A failed read
//!   never fails the poller.
//! - `1`, `true` or `serve`: geo-db answers. The poller scans group 0 from the tick's start time
//!   (the persisted cursor backed up by the overlap window) with an inclusive start, so the rows at
//!   exactly that time are read again; the outbox's `already_notified` (still a Postgres read of
//!   `notification_outbox`) drops the repeats, as it does for the overlap window already. Further
//!   pages of the tick resume after the last row read, which carries its key, so a run of equal
//!   times loses and repeats nothing. A read that fails is an error for the tick, as a Postgres
//!   error is: the cursor does not move and the next tick retries. There is no silent fallback.
//!
//! Collection 33 needs a geodb client and serve at or after geo-db 19b75f0596. Rows of 33 only exist
//! once the vote indexer that writes them has run (its start-up copy backfills every row).
//!
//! What geo-db does not carry is `votes_count.id`, the keyset tiebreaker, so a row read here has
//! `cursor_id` 0. The poller's high-water mark is then `(time, 0)`, which still orders by time, and
//! a Postgres-served tick after a geo-db-served one resumes from that time, re-reading the rows at
//! it. The live mirror stamps a row with the wall clock when it writes, which is within a batch's
//! time of Postgres' own `updated_at`; the overlap window already covers that skew.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use geodb_cursor::derived_read::{cursor_after, cursor_at_score, DerivedReader, SCAN_MAX};
use geodb_cursor::{Config, MirrorError};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::StorageError;
use crate::storage::{EntityVoteCount, Storage};

pub const FLAG: &str = "NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES";
pub const TOTAL: &str = "notification_indexer_geodb_vote_updates_total";

/// `votes_count_updates` in geo-db's collection registry.
pub const COLLECTION: u32 = 33;
/// The poller's `object_type = 0 AND vote_kind = 0` as collection 33's group (`voteUpdateGroup(0, 0)`).
pub const POLLER_GROUP: u128 = 0;
/// The bytes of a collection 33 key (and of 26's): object id, Space id, object type, vote kind.
const KEY_LEN: usize = 34;
/// The bytes of a collection 33 value: `positive`, `negative`, `updated_us`.
const VALUE_LEN: usize = 24;
/// How far apart Postgres' `updated_at` and the mirror's stamp of a row may be, in microseconds,
/// for a shadow comparison to still treat them as one time. The live mirror stamps at its write,
/// after the commit.
pub const SKEW_US: i64 = 5_000_000;

/// What the flag asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Shadow,
    Serve,
}

impl Mode {
    pub fn parse(v: Option<&str>) -> Mode {
        match v.map(str::trim) {
            Some("shadow") => Mode::Shadow,
            Some("1") | Some("true") | Some("serve") => Mode::Serve,
            _ => Mode::Off,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VoteUpdatesError {
    #[error("geo-db: {0}")]
    Geodb(#[from] MirrorError),
    #[error("a collection 33 row is malformed: {0}")]
    Malformed(&'static str),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// One row of collection 33, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub upvotes: i64,
    pub downvotes: i64,
    pub updated_us: i64,
}

/// Decodes one scan row: the 34-byte key and 24-byte value of collection 26's row.
pub fn decode(key: &[u8], value: &[u8]) -> Result<Update, VoteUpdatesError> {
    if key.len() != KEY_LEN {
        return Err(VoteUpdatesError::Malformed("key is not 34 bytes"));
    }
    if value.len() != VALUE_LEN {
        return Err(VoteUpdatesError::Malformed("value is not 24 bytes"));
    }
    let i64_at = |b: &[u8], at: usize| i64::from_le_bytes(b[at..at + 8].try_into().expect("8"));
    Ok(Update {
        entity_id: Uuid::from_slice(&key[0..16]).expect("16"),
        space_id: Uuid::from_slice(&key[16..32]).expect("16"),
        upvotes: i64_at(value, 0),
        downvotes: i64_at(value, 8),
        updated_us: i64_at(value, 16),
    })
}

/// Collection 33's score for an update time: its negation (`voteUpdateScore`).
fn score_of_us(us: i64) -> f64 {
    -(us as f64)
}

/// Where a read starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// The rows updated at or after this time (`voteUpdatesFrom(T, inclusive = true)`).
    Time(i64),
    /// The rows after this encoded cursor, the `resume` of an earlier read.
    After(Vec<u8>),
}

/// What one read returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    /// Oldest update first, equal times by key.
    pub updates: Vec<Update>,
    /// The start of the next read: after the last row returned. `None` when no row was returned.
    pub resume: Option<Vec<u8>>,
}

pub struct VoteUpdates {
    reader: DerivedReader,
    mode: Mode,
}

impl VoteUpdates {
    pub fn new(reader: DerivedReader, mode: Mode) -> Self {
        Self { reader, mode }
    }

    /// `None` when the flag is off (the default), or on without `GEODB_KEY_PATH` and
    /// `GEODB_BINDING_PATH`.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let mode = Mode::parse(get(FLAG).as_deref());
        if mode == Mode::Off {
            return None;
        }
        // `Config` keys its own on-switch on `1`; this flag's value is the mode.
        let cfg = Config::from_vars_for(FLAG, |k| {
            if k == FLAG {
                Some("1".to_string())
            } else {
                get(k)
            }
        })?;
        describe();
        Some(Self::new(DerivedReader::new(cfg), mode))
    }

    pub fn from_env() -> Option<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn serves(&self) -> bool {
        self.mode == Mode::Serve
    }

    pub fn shadows(&self) -> bool {
        self.mode == Mode::Shadow
    }

    /// Up to `limit` rows of group 0 from `start`, in as many scan pages as that takes (a page holds
    /// at most [`SCAN_MAX`] rows, and the poller's batch is larger).
    pub async fn read(&self, start: &Start, limit: usize) -> Result<Read, VoteUpdatesError> {
        let mut after: Vec<u8> = match start {
            Start::Time(us) => cursor_at_score(score_of_us(*us), true).ok_or(
                VoteUpdatesError::Malformed("start time is not a finite score"),
            )?,
            Start::After(c) => c.clone(),
        };
        let mut updates: Vec<Update> = Vec::new();
        let mut resume = None;
        while updates.len() < limit {
            let first = (limit - updates.len()).min(SCAN_MAX);
            let page = self
                .reader
                .scan_after(COLLECTION, POLLER_GROUP, first, Some(&after))
                .await?;
            let Some(last) = page.rows.last() else {
                break;
            };
            let next = cursor_after(last.score, &last.key)
                .ok_or(VoteUpdatesError::Malformed("row has no cursor"))?;
            for r in &page.rows {
                updates.push(decode(&r.key, &r.value)?);
            }
            resume = Some(next.clone());
            if !page.more {
                break;
            }
            after = next;
        }
        Ok(Read { updates, resume })
    }

    /// The geo-db-served `find_entity_vote_counts_since`: a page of up to `limit` rows as the poller
    /// reads them. The first page of a tick has `resume` `None` and starts at `start_us`; the caller
    /// keeps the returned cursor in `resume` for the tick's next page. `already_notified` is read
    /// from Postgres for the rows that reach the threshold, the only rows the poller uses it for.
    pub async fn find_since(
        &self,
        storage: &Storage,
        resume: &mut Option<Vec<u8>>,
        start_us: i64,
        threshold: i64,
        limit: i64,
    ) -> Result<Vec<EntityVoteCount>, VoteUpdatesError> {
        let start = match resume.as_ref() {
            Some(c) => Start::After(c.clone()),
            None => Start::Time(start_us),
        };
        let read = self.read(&start, limit.max(1) as usize).await?;
        let mut out = Vec::with_capacity(read.updates.len());
        for u in &read.updates {
            let already_notified = if u.upvotes >= threshold {
                storage
                    .vote_threshold_already_notified(u.entity_id, u.space_id, threshold)
                    .await?
            } else {
                false
            };
            out.push(EntityVoteCount {
                cursor_id: 0,
                entity_id: u.entity_id,
                space_id: u.space_id,
                upvotes: u.upvotes,
                downvotes: u.downvotes,
                updated_at: DateTime::<Utc>::from_timestamp_micros(u.updated_us)
                    .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
                already_notified,
            });
        }
        if read.resume.is_some() {
            *resume = read.resume;
        }
        Ok(out)
    }

    /// Reads the first page from `start_us` and compares it with `pg`, the page Postgres answered
    /// from the same start. Counts the result; never fails the caller.
    pub async fn observe(&self, pg: &[EntityVoteCount], start_us: i64, limit: i64) {
        let limit = limit.max(1) as usize;
        match self.read(&Start::Time(start_us), limit).await {
            Ok(read) => {
                let d = compare(pg, &read.updates, start_us, limit);
                if d.is_match() {
                    metrics::counter!(TOTAL, "result" => "match").increment(1);
                    info!(
                        event = "geodb_vote_updates_shadow_ok",
                        compared = d.compared,
                        "geo-db votes_count_updates matches Postgres"
                    );
                } else {
                    metrics::counter!(TOTAL, "result" => "mismatch").increment(1);
                    warn!(
                        event = "geodb_vote_updates_shadow_mismatch",
                        compared = d.compared,
                        missing_in_geodb = d.missing_in_geodb,
                        extra_in_geodb = d.extra_in_geodb,
                        tally_differs = d.tally_differs,
                        "geo-db votes_count_updates differs from Postgres"
                    );
                }
            }
            Err(e) => {
                metrics::counter!(TOTAL, "result" => "failed").increment(1);
                warn!(event = "geodb_vote_updates_shadow.read_failed", error = %e, "shadow read failed");
            }
        }
    }
}

pub fn describe() {
    metrics::describe_counter!(
        TOTAL,
        "Vote-poller first pages compared with geo-db's votes_count_updates, by result"
    );
    for result in ["match", "mismatch", "failed"] {
        metrics::counter!(TOTAL, "result" => result).absolute(0);
    }
}

/// How two pages of the same read differ, by count. No ids.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Diff {
    /// Keys judged: the keys either page holds inside the comparison window.
    pub compared: usize,
    pub missing_in_geodb: usize,
    pub extra_in_geodb: usize,
    pub tally_differs: usize,
}

impl Diff {
    pub fn is_match(&self) -> bool {
        self.missing_in_geodb == 0 && self.extra_in_geodb == 0 && self.tally_differs == 0
    }
}

/// Compares Postgres' page `pg` with geo-db's `geo`, both read from `start_us` with `limit`.
///
/// The two do not break ties alike (Postgres by id, geo-db by key) and do not stamp a row with the
/// same microsecond ([`SKEW_US`]), so only a window both pages cover is judged: rows stamped from
/// `start_us + SKEW_US` up to `SKEW_US` before the last row of a page that hit `limit` (the earlier
/// last row if both did). A page that did not hit the limit holds everything past the start. A row
/// is judged on its (entity, Space) and its two tallies, each side by its own stamp.
pub fn compare(pg: &[EntityVoteCount], geo: &[Update], start_us: i64, limit: usize) -> Diff {
    let lo = start_us.saturating_add(SKEW_US);
    let mut hi = i64::MAX;
    if pg.len() >= limit {
        if let Some(l) = pg.last() {
            hi = hi.min(l.updated_at.timestamp_micros().saturating_sub(SKEW_US));
        }
    }
    if geo.len() >= limit {
        if let Some(l) = geo.last() {
            hi = hi.min(l.updated_us.saturating_sub(SKEW_US));
        }
    }
    let inside = |us: i64| us >= lo && us < hi;

    let mut pg_rows: HashMap<(Uuid, Uuid), (i64, i64)> = HashMap::new();
    for r in pg {
        if inside(r.updated_at.timestamp_micros()) {
            pg_rows.insert((r.entity_id, r.space_id), (r.upvotes, r.downvotes));
        }
    }
    let mut geo_rows: HashMap<(Uuid, Uuid), (i64, i64)> = HashMap::new();
    for r in geo {
        if inside(r.updated_us) {
            geo_rows.insert((r.entity_id, r.space_id), (r.upvotes, r.downvotes));
        }
    }
    let mut d = Diff::default();
    let mut keys: Vec<&(Uuid, Uuid)> = pg_rows.keys().chain(geo_rows.keys()).collect();
    keys.sort();
    keys.dedup();
    d.compared = keys.len();
    for k in keys {
        match (pg_rows.get(k), geo_rows.get(k)) {
            (Some(_), None) => d.missing_in_geodb += 1,
            (None, Some(_)) => d.extra_in_geodb += 1,
            (Some(a), Some(b)) if a != b => d.tally_differs += 1,
            _ => {}
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use geodb_cursor::derived_read::DerivedReader;
    use std::time::Duration;

    fn u(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    fn key(entity: u8, space: u8) -> Vec<u8> {
        let mut k = Vec::new();
        k.extend_from_slice(u(entity).as_bytes());
        k.extend_from_slice(u(space).as_bytes());
        k.extend_from_slice(&[0, 0]);
        k
    }

    fn value(up: i64, down: i64, us: i64) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&up.to_le_bytes());
        v.extend_from_slice(&down.to_le_bytes());
        v.extend_from_slice(&us.to_le_bytes());
        v
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// One scan line as the pipe prints it: score, key hex, value hex.
    fn line(entity: u8, up: i64, us: i64) -> String {
        format!(
            "-{us}\t{}\t{}\n",
            hex(&key(entity, 7)),
            hex(&value(up, 0, us))
        )
    }

    fn pg_row(entity: u8, up: i64, us: i64) -> EntityVoteCount {
        EntityVoteCount {
            cursor_id: 1,
            entity_id: u(entity),
            space_id: u(7),
            upvotes: up,
            downvotes: 0,
            updated_at: DateTime::<Utc>::from_timestamp_micros(us).unwrap(),
            already_notified: false,
        }
    }

    fn geo_row(entity: u8, up: i64, us: i64) -> Update {
        Update {
            entity_id: u(entity),
            space_id: u(7),
            upvotes: up,
            downvotes: 0,
            updated_us: us,
        }
    }

    fn vars(flag: Option<&str>, paths: bool) -> impl Fn(&str) -> Option<String> {
        let flag = flag.map(str::to_string);
        move |k| match k {
            FLAG => flag.clone(),
            "GEODB_KEY_PATH" if paths => Some("/k".into()),
            "GEODB_BINDING_PATH" if paths => Some("/b".into()),
            _ => None,
        }
    }

    #[test]
    fn off_by_default_and_the_flag_value_is_the_mode() {
        assert_eq!(Mode::parse(None), Mode::Off);
        assert_eq!(Mode::parse(Some("0")), Mode::Off);
        assert_eq!(Mode::parse(Some("yes")), Mode::Off);
        assert_eq!(Mode::parse(Some("shadow")), Mode::Shadow);
        assert_eq!(Mode::parse(Some("1")), Mode::Serve);
        assert_eq!(Mode::parse(Some("true")), Mode::Serve);
        assert_eq!(Mode::parse(Some("serve")), Mode::Serve);
        assert!(VoteUpdates::from_vars(vars(None, true)).is_none());
        assert!(VoteUpdates::from_vars(vars(Some("shadow"), false)).is_none());
        let s = VoteUpdates::from_vars(vars(Some("shadow"), true)).unwrap();
        assert!(s.shadows() && !s.serves());
        let v = VoteUpdates::from_vars(vars(Some("serve"), true)).unwrap();
        assert!(v.serves() && !v.shadows());
    }

    #[test]
    fn a_row_decodes_from_the_key_and_value_of_collection_26() {
        let got = decode(&key(3, 7), &value(5, 2, 1_793_000_000_000_000)).unwrap();
        assert_eq!(
            got,
            Update {
                entity_id: u(3),
                space_id: u(7),
                upvotes: 5,
                downvotes: 2,
                updated_us: 1_793_000_000_000_000,
            }
        );
        assert!(decode(&[0; 33], &value(0, 0, 0)).is_err());
        assert!(decode(&key(1, 1), &[0; 23]).is_err());
    }

    fn fake(script: &str) -> VoteUpdates {
        VoteUpdates::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            Mode::Serve,
        )
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("vote-updates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// A pipe that logs each request and answers with `body` (a printf format, `\n` escaped).
    fn answering(log: &std::path::Path, body: &str) -> String {
        let n = body.replace("\\n", "\n").replace("\\t", "\t").len();
        format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+{n}\\n{body}'; done",
            log.display()
        )
    }

    #[tokio::test]
    async fn a_read_starts_inclusive_at_the_time_and_scans_group_zero() {
        let log = scratch("start");
        let t = 1_793_000_000_000_000_i64;
        let body = format!("{}{}", line(1, 3, t), line(2, 9, t + 4)).replace('\n', "\\n");
        let r = fake(&answering(&log, &body));
        let read = r.read(&Start::Time(t), 1000).await.unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        // Group 0 of collection 33, first 512 (the page maximum, below the 1000 asked), one page,
        // then the cursor: tag 1, the score key of minus t (little-endian), key length 1, key 00.
        let sk = geodb_cursor::derived_read::score_key(-(t as f64)).unwrap();
        let mut cur = vec![1u8];
        cur.extend_from_slice(&sk.to_le_bytes());
        cur.extend_from_slice(&[1, 0]);
        assert_eq!(sent, format!("scan\t33\t0x0\t512\t1\t{}\n", hex(&cur)));
        assert_eq!(read.updates.len(), 2);
        assert_eq!(
            read.updates[0],
            Update {
                entity_id: u(1),
                space_id: u(7),
                upvotes: 3,
                downvotes: 0,
                updated_us: t,
            }
        );
        assert_eq!(read.updates[1].updated_us, t + 4);
        // Resume: after the last row, which carries its score and key.
        assert_eq!(
            read.resume,
            geodb_cursor::derived_read::cursor_after(-((t + 4) as f64), &key(2, 7))
        );
    }

    #[tokio::test]
    async fn a_read_pages_through_more_until_the_limit_resuming_after_each_page() {
        let log = scratch("pages");
        let t = 1_793_000_000_000_000_i64;
        // Every answer is one row and `more`: the read asks again, after that row, until it has 3.
        let body = format!("{}more\\n", line(1, 3, t).replace('\n', "\\n"));
        let r = fake(&answering(&log, &body));
        let read = r.read(&Start::Time(t), 3).await.unwrap();
        let sent = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        let lines: Vec<&str> = sent.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(read.updates.len(), 3);
        // Page sizes shrink to what is left: 3, 2, 1.
        assert!(lines[0].starts_with("scan\t33\t0x0\t3\t1\t"));
        assert!(lines[1].starts_with("scan\t33\t0x0\t2\t1\t"));
        assert!(lines[2].starts_with("scan\t33\t0x0\t1\t1\t"));
        // The second request resumes after the first page's last row, not at the time.
        let after_row =
            hex(&geodb_cursor::derived_read::cursor_after(-(t as f64), &key(1, 7)).unwrap());
        assert!(lines[1].ends_with(&after_row), "{}", lines[1]);
    }

    #[tokio::test]
    async fn a_read_with_no_rows_has_no_resume_and_a_refusal_is_an_error() {
        let log = scratch("empty");
        let r = fake(&answering(&log, ""));
        let read = r.read(&Start::After(vec![1; 11]), 10).await.unwrap();
        let _ = std::fs::remove_file(&log);
        assert!(read.updates.is_empty() && read.resume.is_none());
        let r = fake("echo ready; read -r l; printf -- '-9\\nrefused!!'; exit 1");
        assert!(r.read(&Start::Time(0), 10).await.is_err());
    }

    #[tokio::test]
    async fn a_malformed_row_is_an_error_not_a_skipped_row() {
        let log = scratch("bad");
        // A 2-byte key.
        let r = fake(&answering(&log, "-5\\t0102\\t00\\n"));
        let e = r.read(&Start::Time(0), 10).await.unwrap_err();
        let _ = std::fs::remove_file(&log);
        assert!(matches!(e, VoteUpdatesError::Malformed(_)), "{e}");
    }

    #[test]
    fn a_page_that_matches_is_a_match_whatever_the_tie_order() {
        let t = 2_000_000_000_000_000_i64;
        let pg = vec![pg_row(2, 5, t + 10_000_000), pg_row(1, 3, t + 10_000_000)];
        let geo = vec![geo_row(1, 3, t + 10_000_001), geo_row(2, 5, t + 10_000_001)];
        let d = compare(&pg, &geo, t, 1000);
        assert!(d.is_match(), "{d:?}");
        assert_eq!(d.compared, 2);
    }

    #[test]
    fn differences_are_counted_by_kind() {
        let t = 2_000_000_000_000_000_i64;
        let at = t + 20_000_000;
        let pg = vec![pg_row(1, 3, at), pg_row(2, 5, at), pg_row(3, 1, at)];
        let geo = vec![geo_row(2, 6, at), geo_row(3, 1, at), geo_row(4, 1, at)];
        let d = compare(&pg, &geo, t, 1000);
        assert_eq!(
            d,
            Diff {
                compared: 4,
                missing_in_geodb: 1,
                extra_in_geodb: 1,
                tally_differs: 1
            }
        );
        assert!(!d.is_match());
    }

    #[test]
    fn rows_within_the_skew_of_the_start_are_not_judged() {
        let t = 2_000_000_000_000_000_i64;
        // Postgres stamped it at the start, the mirror a moment before: geo-db's read from the start
        // does not return it. Not a gap.
        let pg = vec![pg_row(1, 3, t + 1_000)];
        assert!(compare(&pg, &[], t, 1000).is_match());
        let geo = vec![geo_row(1, 3, t + 1_000)];
        assert!(compare(&[], &geo, t, 1000).is_match());
    }

    #[test]
    fn a_full_page_is_judged_only_up_to_just_before_its_last_row() {
        let t = 2_000_000_000_000_000_i64;
        let a = t + 10_000_000;
        let z = t + 60_000_000;
        // Postgres' page is full (limit 2) and ends at z; geo-db's page is the same rows past the
        // limit's tie-break differences: geo-db holds a row Postgres' cut left off, after z.
        let pg = vec![pg_row(1, 3, a), pg_row(2, 4, z)];
        let geo = vec![
            geo_row(1, 3, a),
            geo_row(2, 4, z),
            geo_row(3, 9, z + 1_000_000),
        ];
        let d = compare(&pg, &geo, t, 2);
        assert!(d.is_match(), "{d:?}");
        // Only the row before the fence was judged.
        assert_eq!(d.compared, 1);
        // Without the limit the extra row is a real difference.
        assert_eq!(compare(&pg, &geo, t, 1000).extra_in_geodb, 1);
    }

    #[tokio::test]
    async fn observe_swallows_a_dead_pipe() {
        let r = fake("exit 1");
        r.observe(&[pg_row(1, 1, 0)], 0, 1000).await;
    }
}
