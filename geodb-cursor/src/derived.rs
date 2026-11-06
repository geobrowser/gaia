//! Best-effort mirror of derived score rows into geo-db's derived class (GAP-9, plan 10.14), over
//! the same held `geodb-client derived pipe` child the cursor mirror uses.
//!
//! Postgres stays the source of truth: a writer finishes its Postgres write, then hands the rows
//! it wrote (and the keys that left) to [`DerivedMirror`]. Nothing here can fail the writer: every
//! error is logged, counted (`geodb_derived_mirror_errors_total{table}`) and swallowed, each
//! request is bounded by the child's timeout, a call is bounded by [`Config`]'s `budget` (the rows
//! past it are counted in `geodb_derived_mirror_dropped_total{table}`, not queued), and after a
//! failure the child is not respawned until the cool-down passes. Rows are last-write-wins, so a
//! dropped row is repaired by the next write of the same key.
//!
//! One row is one `put` request line (the pipe carries one row a line; each is one fsynced frame
//! in geo-db), so this suits incremental writers (a vote batch, a block's recompute). A table that
//! rewrites millions of rows in one pass needs a batching pipe verb first.
//!
//! Keys and values travel as `hex:` so a tab or newline in them cannot break the line. A row with
//! a key over `KEY_MAX` (48), a value over `VALUE_MAX` (64) or a non-finite score is skipped and
//! counted (`geodb_derived_mirror_skipped_total{table}`), never truncated.
//!
//! Off unless `GEODB_DERIVED_MIRROR` is `1` or `true`; it needs `GEODB_KEY_PATH` and
//! `GEODB_BINDING_PATH` like the cursor mirror, and the key's binding must grant `derived=` for the
//! collections written.
//!
//! `GEODB_DERIVED_MIRROR_TABLES` (comma separated table labels, e.g.
//! `entity_type_ranking,entity_topic_ranking`) narrows a service to those tables, because the flag
//! is one switch per service and some services write a table far larger than the rest (the
//! ranking-indexer's `ranking_scores`). Unset, empty or only commas keeps every table; a list that
//! names no table the service writes mirrors nothing. It never turns the mirror on by itself.

use std::time::{Duration, Instant};

use tracing::{error, info, warn};

use crate::{Config, CursorMirror, MirrorError};

/// Most bytes of a derived key (`row.key_max`).
pub const KEY_MAX: usize = 48;
/// Most bytes of a derived value outside the `cursors` collection (`row.value_max`).
pub const VALUE_MAX: usize = 64;
/// Requests sent between two looks at the clock.
const CHUNK: usize = 64;
/// Rows in one batch request (`wire.derived.batch_max`).
pub const BATCH_MAX: usize = 256;

/// One row of a derived collection: `(collection, key) -> (group, score, value)`.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedRow {
    pub collection: u32,
    pub group: u128,
    pub score: f64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl DerivedRow {
    /// The pipe's request line, or why the row cannot be sent.
    pub fn put_line(&self) -> Result<String, MirrorError> {
        if self.key.is_empty() || self.key.len() > KEY_MAX {
            return Err(MirrorError::Refused(format!(
                "key of {} bytes",
                self.key.len()
            )));
        }
        if self.value.len() > VALUE_MAX {
            return Err(MirrorError::TooLong(self.value.len()));
        }
        if !self.score.is_finite() {
            return Err(MirrorError::Refused("score is not finite".into()));
        }
        // `{:?}` of an f64 is the shortest form that parses back to the same value.
        Ok(format!(
            "put\t{}\t{}\t{:?}\thex:{}\thex:{}\n",
            self.collection,
            self.group,
            self.score,
            hex(&self.key),
            hex(&self.value)
        ))
    }
}

/// The delete request line for `key` in `collection`.
pub fn delete_line(collection: u32, key: &[u8]) -> Result<String, MirrorError> {
    if key.is_empty() || key.len() > KEY_MAX {
        return Err(MirrorError::Refused(format!("key of {} bytes", key.len())));
    }
    Ok(format!("delete\t{collection}\thex:{}\n", hex(key)))
}

/// Holds the `derived pipe` child. Cheap to build; the child starts on first use.
pub struct DerivedMirror {
    pipe: CursorMirror,
    budget: Duration,
    batch: bool,
    /// `None` mirrors every table; `Some` only the tables named (`GEODB_DERIVED_MIRROR_TABLES`).
    tables: Option<std::collections::HashSet<String>>,
}

/// One batch request from `lines` of a single kind: `putn<tab><n>` (or `deleten`), then the rows.
/// The pipe answers it with one frame, and the node commits the rows as one frame (one fsync).
/// A pipe child older than the batch verbs would answer the head `-` and each row separately, so
/// this is only sent when `GEODB_DERIVED_BATCH` is on.
pub fn batch_request(lines: &[String]) -> String {
    let head = if lines[0].starts_with("put\t") {
        "putn"
    } else {
        "deleten"
    };
    let mut out = format!("{head}\t{}\n", lines.len());
    for l in lines {
        out.push_str(l);
    }
    out
}

impl DerivedMirror {
    pub fn new(cfg: Config, budget: Duration) -> Self {
        Self {
            pipe: CursorMirror::new(cfg),
            budget,
            batch: false,
            tables: None,
        }
    }

    /// Restricts the mirror to the tables named (the `table` label every call carries). `None`, or
    /// an empty list, mirrors all of them. A table off the list is neither sent nor counted as a
    /// put, delete, error or drop: it only increments `geodb_derived_mirror_filtered_total{table}`.
    pub fn with_tables(mut self, tables: Option<&[&str]>) -> Self {
        self.tables = tables
            .filter(|t| !t.is_empty())
            .map(|t| t.iter().map(|n| n.to_string()).collect());
        self
    }

    /// True when `table` is on the allow list, or there is none.
    pub fn allows(&self, table: &str) -> bool {
        self.tables.as_ref().is_none_or(|t| t.contains(table))
    }

    fn filtered(&self, table: &str) -> bool {
        if self.allows(table) {
            return false;
        }
        metrics::counter!("geodb_derived_mirror_filtered_total", "table" => table.to_string())
            .increment(1);
        true
    }

    /// Sends up to [`BATCH_MAX`] rows a request (`putn` / `deleten`) instead of one. Needs a
    /// `geodb-client` that has the batch verbs.
    pub fn with_batch(mut self, on: bool) -> Self {
        self.batch = on;
        self
    }

    /// `Some` only when `GEODB_DERIVED_MIRROR` is on and configured. `GEODB_DERIVED_BUDGET_MS`
    /// (default 5000) bounds one call.
    pub fn from_env() -> Option<Self> {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let cfg = Config::from_vars_for("GEODB_DERIVED_MIRROR", &get)?;
        let ms = get("GEODB_DERIVED_BUDGET_MS")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(5000);
        let batch = matches!(
            get("GEODB_DERIVED_BATCH").as_deref().map(str::trim),
            Some("1") | Some("true")
        );
        // Comma separated table labels; unset, empty or only commas means every table. A value that
        // names no table the writers use mirrors nothing (the flag above still gates everything).
        let names: Vec<String> = get("GEODB_DERIVED_MIRROR_TABLES")
            .unwrap_or_default()
            .split(',')
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        Some(
            Self::new(cfg, Duration::from_millis(ms))
                .with_batch(batch)
                .with_tables(Some(&refs)),
        )
    }

    /// Mirrors `rows` of `table` (a label for the metrics). Never fails the caller.
    pub async fn put(&self, table: &str, rows: &[DerivedRow]) {
        self.put_counted(table, rows).await;
    }

    /// As [`put`](Self::put), and returns how many rows the node acknowledged. A one-shot backfill
    /// uses it to tell a short send (dead pipe, cooldown, budget, an unencodable row) from a full one.
    pub async fn put_counted(&self, table: &str, rows: &[DerivedRow]) -> usize {
        if self.filtered(table) {
            return 0;
        }
        let mut lines = Vec::with_capacity(rows.len());
        for r in rows {
            match r.put_line() {
                Ok(l) => lines.push(l),
                Err(e) => self.skipped(table, &e),
            }
        }
        let sent = self.send(table, &lines).await;
        metrics::counter!("geodb_derived_mirror_puts_total", "table" => table.to_string())
            .increment(sent as u64);
        self.report("put", table, rows.len(), sent);
        sent
    }

    /// Logs how many rows of a call geo-db acknowledged, and an error (never a failed exit) with a
    /// counter (`geodb_derived_mirror_unacknowledged_total{table}`) when fewer than were asked.
    /// A CronJob's Prometheus samples are gone with its pod, so the log line is what an operator
    /// has; it is emitted whatever the run's `RUST_LOG` allows for the caller's own target.
    fn report(&self, op: &str, table: &str, requested: usize, acknowledged: usize) {
        if requested == 0 {
            return;
        }
        if acknowledged < requested {
            let short = (requested - acknowledged) as u64;
            metrics::counter!("geodb_derived_mirror_unacknowledged_total", "table" => table.to_string())
                .increment(short);
            error!(
                event = "geodb_derived.not_acknowledged",
                op,
                table,
                requested,
                acknowledged,
                "geo-db did not acknowledge every row; Postgres is unaffected, geo-db is behind"
            );
        } else {
            info!(
                event = "geodb_derived.acknowledged",
                op, table, requested, acknowledged, "geo-db acknowledged the rows"
            );
        }
    }

    /// Mirrors the removal of `keys` of `collection` (`table` labels the metrics).
    pub async fn delete(&self, table: &str, collection: u32, keys: &[Vec<u8>]) {
        if self.filtered(table) {
            return;
        }
        let mut lines = Vec::with_capacity(keys.len());
        for k in keys {
            match delete_line(collection, k) {
                Ok(l) => lines.push(l),
                Err(e) => self.skipped(table, &e),
            }
        }
        let sent = self.send(table, &lines).await;
        metrics::counter!("geodb_derived_mirror_deletes_total", "table" => table.to_string())
            .increment(sent as u64);
        self.report("delete", table, keys.len(), sent);
    }

    fn skipped(&self, table: &str, e: &MirrorError) {
        metrics::counter!("geodb_derived_mirror_skipped_total", "table" => table.to_string())
            .increment(1);
        warn!(event = "geodb_derived.row_skipped", table, error = %e, "row cannot be mirrored; skipped");
    }

    /// Sends `lines` in chunks while the budget holds; returns how many were answered `+`.
    async fn send(&self, table: &str, lines: &[String]) -> usize {
        let start = Instant::now();
        let mut sent = 0;
        let size = if self.batch { BATCH_MAX } else { CHUNK };
        for chunk in lines.chunks(size) {
            if sent > 0 && start.elapsed() > self.budget {
                let dropped = lines.len() - sent;
                metrics::counter!("geodb_derived_mirror_dropped_total", "table" => table.to_string())
                    .increment(dropped as u64);
                warn!(
                    event = "geodb_derived.budget_spent",
                    table, dropped, "mirror budget spent; the rest is left for the next write"
                );
                break;
            }
            let one;
            let reqs: &[String] = if self.batch && chunk.len() > 1 {
                one = [batch_request(chunk)];
                &one
            } else {
                chunk
            };
            if let Err(e) = self.pipe.send_lines(reqs).await {
                if !matches!(e, MirrorError::CoolingDown) {
                    // error level, and a message that names the cause: a refusal used to be a warn on
                    // a target the CronJobs' RUST_LOG hid, so a run that wrote nothing looked clean.
                    error!(event = "geodb_derived.mirror_failed", table, error = %e, "geo-db derived mirror failed; Postgres is unaffected");
                }
                metrics::counter!("geodb_derived_mirror_errors_total", "table" => table.to_string())
                    .increment(1);
                metrics::counter!("geodb_derived_mirror_dropped_total", "table" => table.to_string())
                    .increment((lines.len() - sent) as u64);
                break;
            }
            sent += chunk.len();
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(script: &str, budget: Duration) -> DerivedMirror {
        DerivedMirror::new(
            Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            },
            budget,
        )
    }

    fn ok_script(log: &std::path::Path) -> String {
        format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+10\\napplied: 1'; done",
            log.display()
        )
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("geodb-derived-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn row(key: u8, score: f64) -> DerivedRow {
        DerivedRow {
            collection: 0,
            group: 0xab,
            score,
            key: vec![key; 32],
            value: vec![],
        }
    }

    #[tokio::test]
    async fn puts_and_deletes_travel_as_hex_lines_in_order() {
        let log = tmp("lines");
        let m = fake(&ok_script(&log), Duration::from_secs(5));
        m.put("t", &[row(1, 2.5), row(2, -1.0)]).await;
        m.delete("t", 0, &[vec![1; 32]]).await;
        let sent = std::fs::read_to_string(&log).unwrap();
        let k1 = "01".repeat(32);
        let k2 = "02".repeat(32);
        assert_eq!(
            sent,
            format!(
                "put\t0\t171\t2.5\thex:{k1}\thex:\nput\t0\t171\t-1.0\thex:{k2}\thex:\ndelete\t0\thex:{k1}\n"
            )
        );
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn bad_rows_are_refused_before_any_io() {
        let mut r = row(1, 1.0);
        r.key = vec![];
        assert!(matches!(r.put_line(), Err(MirrorError::Refused(_))));
        r.key = vec![0; 49];
        assert!(matches!(r.put_line(), Err(MirrorError::Refused(_))));
        r.key = vec![0; 48];
        assert!(r.put_line().is_ok());
        r.value = vec![0; 65];
        assert!(matches!(r.put_line(), Err(MirrorError::TooLong(65))));
        r.value = vec![0; 64];
        assert!(r.put_line().is_ok());
        assert!(matches!(
            row(1, f64::NAN).put_line(),
            Err(MirrorError::Refused(_))
        ));
        assert!(matches!(
            row(1, f64::INFINITY).put_line(),
            Err(MirrorError::Refused(_))
        ));
        assert!(matches!(delete_line(0, &[]), Err(MirrorError::Refused(_))));
    }

    #[tokio::test]
    async fn put_counted_reports_rows_acknowledged() {
        let log = tmp("counted");
        let m = fake(&ok_script(&log), Duration::from_secs(5));
        assert_eq!(m.put_counted("t", &[row(1, 1.0), row(2, 2.0)]).await, 2);
        // A row that cannot be encoded is skipped, so the count falls short of the input.
        assert_eq!(
            m.put_counted("t", &[row(3, 1.0), row(4, f64::NAN)]).await,
            1
        );
        let dead = fake("exit 1", Duration::from_secs(5));
        assert_eq!(dead.put_counted("t", &[row(5, 1.0)]).await, 0);
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn a_refusal_ends_the_batch_and_is_swallowed() {
        let log = tmp("refuse");
        // Answers the first request `-`, exits: the second row is never sent.
        let script = format!(
            "echo ready; read -r l; printf '%s\\n' \"$l\" >> {}; printf -- '-26\\nrefused: DefiniteRefusal X'; exit 1",
            log.display()
        );
        let m = fake(&script, Duration::from_secs(5));
        m.put("t", &[row(1, 1.0), row(2, 2.0)]).await; // must not panic or error
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
        // Cooling down: the next call sends nothing and still returns.
        m.put("t", &[row(3, 3.0)]).await;
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn a_batch_request_is_a_head_then_the_rows() {
        let p = vec![
            "put\t0\t1\t1.0\thex:aa\thex:\n".to_string(),
            "put\t0\t1\t2.0\thex:bb\thex:\n".to_string(),
        ];
        assert_eq!(batch_request(&p), format!("putn\t2\n{}{}", p[0], p[1]));
        let d = vec!["delete\t0\thex:aa\n".to_string()];
        assert_eq!(batch_request(&d), "deleten\t1\ndelete\t0\thex:aa\n");
    }

    #[tokio::test]
    async fn batching_sends_one_request_per_256_rows_and_counts_them_sent() {
        let log = tmp("batch");
        // One answer per request, however many lines it holds: it reads the head, then n rows.
        let script = format!(
            "echo ready; while IFS= read -r h; do n=${{h#*\t}}; printf '%s\\n' \"$h\" >> {0}; i=0; while [ $i -lt $n ]; do IFS= read -r r; i=$((i+1)); done; printf '+10\\napplied: 1'; done",
            log.display()
        );
        let m = fake(&script, Duration::from_secs(5)).with_batch(true);
        let more: Vec<_> = (0..(BATCH_MAX as u16 + 10))
            .map(|i| DerivedRow {
                key: i.to_be_bytes().to_vec(),
                ..row(0, 1.0)
            })
            .collect();
        m.put("t", &more).await;
        let heads = std::fs::read_to_string(&log).unwrap();
        assert_eq!(heads, "putn\t256\nputn\t10\n");
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn a_spent_budget_leaves_the_rest_unsent() {
        let log = tmp("budget");
        let m = fake(&ok_script(&log), Duration::ZERO);
        let rows: Vec<_> = (0..(CHUNK as u8 + 6)).map(|i| row(i, 1.0)).collect();
        m.put("t", &rows).await;
        // Zero budget: the first chunk is sent (the clock is looked at between chunks), the rest not.
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().lines().count(),
            CHUNK
        );
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn missing_binary_is_swallowed() {
        let m = DerivedMirror::new(
            Config {
                program: "/nonexistent/geodb-client".into(),
                args: vec![],
                collection: 0,
                timeout: Duration::from_secs(1),
                cooldown: Duration::ZERO,
            },
            Duration::from_secs(1),
        );
        m.put("t", &[row(1, 1.0)]).await;
        m.delete("t", 0, &[vec![1]]).await;
    }

    #[test]
    fn flag_defaults_off_and_is_its_own_switch() {
        assert!(DerivedMirror::from_vars(|_| None).is_none());
        // The cursor mirror's switch does not turn this one on.
        let cursor_only = |k: &str| match k {
            "GEODB_CURSOR_MIRROR" => Some("1".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        };
        assert!(DerivedMirror::from_vars(cursor_only).is_none());
        let on = |k: &str| match k {
            "GEODB_DERIVED_MIRROR" => Some("true".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            _ => None,
        };
        assert!(DerivedMirror::from_vars(on).is_some());
        let no_paths = |k: &str| (k == "GEODB_DERIVED_MIRROR").then(|| "1".to_string());
        assert!(DerivedMirror::from_vars(no_paths).is_none());
    }

    fn on_vars(tables: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
        move |k: &str| match k {
            "GEODB_DERIVED_MIRROR" => Some("1".to_string()),
            "GEODB_KEY_PATH" => Some("/k".into()),
            "GEODB_BINDING_PATH" => Some("/b".into()),
            "GEODB_DERIVED_MIRROR_TABLES" => tables.map(str::to_string),
            _ => None,
        }
    }

    #[test]
    fn table_list_unset_or_blank_allows_every_table() {
        for t in [None, Some(""), Some("  "), Some(" , ,")] {
            let m = DerivedMirror::from_vars(on_vars(t)).unwrap();
            assert!(m.allows("ranking_scores"), "{t:?}");
            assert!(m.allows("entity_type_ranking"), "{t:?}");
        }
    }

    #[test]
    fn table_list_narrows_to_the_named_tables() {
        let m =
            DerivedMirror::from_vars(on_vars(Some("entity_type_ranking, entity_topic_ranking")))
                .unwrap();
        assert!(m.allows("entity_type_ranking"));
        assert!(m.allows("entity_topic_ranking"));
        assert!(!m.allows("ranking_scores"));
        assert!(!m.allows("votes_count"));
    }

    #[test]
    fn a_list_naming_nothing_known_mirrors_nothing_and_the_flag_still_gates() {
        let m = DerivedMirror::from_vars(on_vars(Some("bogus"))).unwrap();
        assert!(!m.allows("ranking_scores"));
        // The list never turns the mirror on by itself.
        let off = |k: &str| (k == "GEODB_DERIVED_MIRROR_TABLES").then(|| "ranking_scores".into());
        assert!(DerivedMirror::from_vars(off).is_none());
    }

    #[tokio::test]
    async fn a_table_off_the_list_sends_nothing_and_counts_zero() {
        let log = tmp("filtered");
        let m = fake(&ok_script(&log), Duration::from_secs(5)).with_tables(Some(&["kept"]));
        assert_eq!(m.put_counted("dropped", &[row(1, 1.0)]).await, 0);
        m.put("dropped", &[row(2, 1.0)]).await;
        m.delete("dropped", 0, &[vec![1; 32]]).await;
        assert!(
            !log.exists(),
            "nothing may reach the pipe for a filtered table"
        );
        assert_eq!(m.put_counted("kept", &[row(3, 1.0)]).await, 1);
        m.delete("kept", 0, &[vec![3; 32]]).await;
        let sent = std::fs::read_to_string(&log).unwrap();
        assert_eq!(sent.lines().count(), 2);
        let _ = std::fs::remove_file(&log);
    }
}
