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

use std::time::{Duration, Instant};

use tracing::warn;

use crate::{Config, CursorMirror, MirrorError};

/// Most bytes of a derived key (`row.key_max`).
pub const KEY_MAX: usize = 48;
/// Most bytes of a derived value outside the `cursors` collection (`row.value_max`).
pub const VALUE_MAX: usize = 64;
/// Requests sent between two looks at the clock.
const CHUNK: usize = 64;

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
}

impl DerivedMirror {
    pub fn new(cfg: Config, budget: Duration) -> Self {
        Self {
            pipe: CursorMirror::new(cfg),
            budget,
        }
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
        Some(Self::new(cfg, Duration::from_millis(ms)))
    }

    /// Mirrors `rows` of `table` (a label for the metrics). Never fails the caller.
    pub async fn put(&self, table: &str, rows: &[DerivedRow]) {
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
    }

    /// Mirrors the removal of `keys` of `collection` (`table` labels the metrics).
    pub async fn delete(&self, table: &str, collection: u32, keys: &[Vec<u8>]) {
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
        for chunk in lines.chunks(CHUNK) {
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
            if let Err(e) = self.pipe.send_lines(chunk).await {
                if !matches!(e, MirrorError::CoolingDown) {
                    warn!(event = "geodb_derived.mirror_failed", table, error = %e, "geo-db derived mirror failed; Postgres is unaffected");
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
}
