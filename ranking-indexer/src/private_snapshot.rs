//! One-shot backfill and a read-only compare for the per-user derived collections 27
//! (`external_interest_signals`), 28 (`user_topic_signals`) and 29 (`feed_user_seen_daily`).
//!
//! Why: the three writers copy rows as they run (`not_interested_sync` every two minutes, the
//! `topic_interest` sweep every two minutes for the last ten minutes and the refit nightly,
//! `feed_signals_sync` hourly and only when analytics published something new), so a row written
//! before the private switch went on can wait up to a day to reach geo-db. The `backfill` mode of
//! `private_mirror_snapshot` reads the three tables whole and puts every row, with the same `plan`
//! functions the writers use, so the layouts cannot drift. The `compare` mode reads geo-db back
//! with the same key (it holds `derived=27,28,29`, so the restriction lets it read) and counts,
//! per table, rows that match, differ, are missing from geo-db or are extra in it.
//!
//! The compare is by user group: for every user Postgres holds rows for, one `scan` of that
//! group (at most [`SCAN_ROWS`] rows). A group with more rows than that is counted `partial` and
//! only the rows the scan returned are judged. A user whose rows Postgres has dropped entirely is
//! not found (nothing names the group); that blind spot is the erasure path, not this check.
//!
//! Nothing here logs a user, topic or item id: counts only.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use geodb_cursor::derived::DerivedRow;

/// Most rows one `scan` page holds (geo-db `wire_derived.page_max`).
pub const SCAN_ROWS: usize = 512;

/// One row geo-db returned from a scan: score, key and value.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub key: Vec<u8>,
    pub score: f64,
    pub value: Vec<u8>,
}

/// What one group (or a whole table) compared to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub matched: u64,
    /// The key is in both and the value or score differs.
    pub mismatched: u64,
    /// Postgres has it and geo-db does not.
    pub missing: u64,
    /// geo-db has it in this user's group and Postgres does not.
    pub extra: u64,
    /// Groups where the scan was cut at the page bound, so `missing` was not judged.
    pub partial_groups: u64,
}

impl Tally {
    pub fn add(&mut self, o: Tally) {
        self.matched += o.matched;
        self.mismatched += o.mismatched;
        self.missing += o.missing;
        self.extra += o.extra;
        self.partial_groups += o.partial_groups;
    }

    /// True when geo-db agrees with Postgres on everything that was judged.
    pub fn clean(&self) -> bool {
        self.mismatched == 0 && self.missing == 0 && self.extra == 0
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The rows a `scan` answer body names, and whether the page was cut: one
/// `<score>\t<key hex>\t<value hex>` a line, then `more` when the group held more.
pub fn parse_scan(body: &str) -> Result<(Vec<Held>, bool), String> {
    let mut rows = Vec::new();
    let mut more = false;
    for line in body.lines() {
        if line.is_empty() {
            continue;
        }
        if line == "more" {
            more = true;
            continue;
        }
        let mut f = line.split('\t');
        let (Some(score), Some(key), Some(value), None) = (f.next(), f.next(), f.next(), f.next())
        else {
            return Err("scan row is malformed".into());
        };
        let score: f64 = score.parse().map_err(|_| "scan score is malformed")?;
        let key = unhex(key)
            .filter(|k| !k.is_empty())
            .ok_or("scan key is malformed")?;
        let value = unhex(value).ok_or("scan value is malformed")?;
        rows.push(Held { key, score, value });
    }
    Ok((rows, more))
}

/// Groups the expected rows by their group, so one scan covers one user.
pub fn by_group(rows: Vec<DerivedRow>) -> BTreeMap<u128, Vec<DerivedRow>> {
    let mut m: BTreeMap<u128, Vec<DerivedRow>> = BTreeMap::new();
    for r in rows {
        m.entry(r.group).or_default().push(r);
    }
    m
}

fn same_score(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Compares one group's expected rows with what its scan returned.
pub fn compare_group(expected: &[DerivedRow], held: &[Held], more: bool) -> Tally {
    let have: HashMap<&[u8], &Held> = held.iter().map(|h| (h.key.as_slice(), h)).collect();
    let want: HashMap<&[u8], &DerivedRow> =
        expected.iter().map(|r| (r.key.as_slice(), r)).collect();
    let mut t = Tally::default();
    for r in expected {
        match have.get(r.key.as_slice()) {
            Some(h) if h.value == r.value && same_score(h.score, r.score) => t.matched += 1,
            Some(_) => t.mismatched += 1,
            None if more => {}
            None => t.missing += 1,
        }
    }
    t.extra = held
        .iter()
        .filter(|h| !want.contains_key(h.key.as_slice()))
        .count() as u64;
    if more {
        t.partial_groups = 1;
    }
    t
}

/// A held `geodb-client derived pipe` child that answers `scan` requests. Blocking.
pub struct ScanPipe {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ScanPipe {
    /// Starts `program` with `args` and waits for its `ready` line.
    pub fn spawn(program: &std::ffi::OsStr, args: &[std::ffi::OsString]) -> Result<Self, String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start geodb-client: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut me = ScanPipe {
            child,
            stdin,
            stdout,
        };
        let mut line = String::new();
        me.stdout
            .read_line(&mut line)
            .map_err(|e| format!("geodb-client gave no ready line: {e}"))?;
        if line.trim_end() != "ready" {
            return Err("geodb-client did not say ready".into());
        }
        Ok(me)
    }

    /// One request line, one frame: the body of a `+` answer, an error for a `-` answer.
    pub fn request(&mut self, line: &str) -> Result<String, String> {
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("geodb-client write failed: {e}"))?;
        let mut head = String::new();
        let n = self
            .stdout
            .read_line(&mut head)
            .map_err(|e| format!("geodb-client read failed: {e}"))?;
        if n == 0 {
            return Err("geodb-client closed".into());
        }
        let head = head.trim_end();
        let (ok, len) = match (head.strip_prefix('+'), head.strip_prefix('-')) {
            (Some(n), _) => (true, n),
            (_, Some(n)) => (false, n),
            _ => return Err("geodb-client frame is malformed".into()),
        };
        let len: usize = len
            .parse()
            .map_err(|_| "geodb-client frame length is malformed")?;
        let mut body = vec![0u8; len];
        self.stdout
            .read_exact(&mut body)
            .map_err(|e| format!("geodb-client frame is short: {e}"))?;
        let body = String::from_utf8_lossy(&body).into_owned();
        if ok {
            Ok(body)
        } else {
            Err(format!(
                "refused: {}",
                body.chars().take(120).collect::<String>()
            ))
        }
    }

    /// Scans one user's group: the rows and whether the page was cut.
    pub fn scan(&mut self, collection: u32, group: u128) -> Result<(Vec<Held>, bool), String> {
        let body = self.request(&format!(
            "scan\t{collection}\t0x{group:032x}\t{SCAN_ROWS}\t1"
        ))?;
        parse_scan(&body)
    }
}

impl Drop for ScanPipe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(group: u128, key: &[u8], score: f64, value: &[u8]) -> DerivedRow {
        DerivedRow {
            collection: 27,
            group,
            score,
            key: key.to_vec(),
            value: value.to_vec(),
        }
    }

    fn held(key: &[u8], score: f64, value: &[u8]) -> Held {
        Held {
            key: key.to_vec(),
            score,
            value: value.to_vec(),
        }
    }

    #[test]
    fn a_scan_body_is_rows_then_an_optional_more() {
        let (rows, more) = parse_scan("1.5\t0a0b\t0102\n0.25\tff\t\nmore\n").unwrap();
        assert!(more);
        assert_eq!(
            rows,
            vec![held(&[10, 11], 1.5, &[1, 2]), held(&[255], 0.25, &[])]
        );
        let (rows, more) = parse_scan("").unwrap();
        assert!(rows.is_empty() && !more);
    }

    #[test]
    fn a_malformed_scan_row_is_an_error() {
        assert!(parse_scan("nope\t0a\t\n").is_err());
        assert!(parse_scan("1.0\t0g\t\n").is_err());
        assert!(parse_scan("1.0\t0a\n").is_err());
        assert!(parse_scan("1.0\t\t\n").is_err());
    }

    #[test]
    fn rows_are_grouped_by_user() {
        let g = by_group(vec![
            row(2, b"a", 1.0, b""),
            row(1, b"b", 1.0, b""),
            row(2, b"c", 1.0, b""),
        ]);
        assert_eq!(g.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(g[&2].len(), 2);
    }

    #[test]
    fn equal_rows_match_and_the_score_tolerates_float_noise() {
        let e = [row(1, b"a", 0.3, b"xy")];
        let h = [held(b"a", 0.1 + 0.2, b"xy")];
        assert_eq!(
            compare_group(&e, &h, false),
            Tally {
                matched: 1,
                ..Tally::default()
            }
        );
    }

    #[test]
    fn a_different_value_or_score_is_a_mismatch() {
        let e = [row(1, b"a", 1.0, b"xy"), row(1, b"b", 1.0, b"")];
        let h = [held(b"a", 1.0, b"xz"), held(b"b", 2.0, b"")];
        let t = compare_group(&e, &h, false);
        assert_eq!((t.matched, t.mismatched), (0, 2));
        assert!(!t.clean());
    }

    #[test]
    fn missing_and_extra_rows_are_counted() {
        let e = [row(1, b"a", 1.0, b""), row(1, b"b", 1.0, b"")];
        let h = [held(b"a", 1.0, b""), held(b"z", 1.0, b"")];
        let t = compare_group(&e, &h, false);
        assert_eq!((t.matched, t.missing, t.extra), (1, 1, 1));
    }

    #[test]
    fn a_cut_scan_does_not_judge_missing_rows() {
        let e = [row(1, b"a", 1.0, b""), row(1, b"b", 1.0, b"")];
        let h = [held(b"a", 1.0, b"")];
        let t = compare_group(&e, &h, true);
        assert_eq!((t.matched, t.missing, t.partial_groups), (1, 0, 1));
        assert!(t.clean());
    }

    #[test]
    fn tallies_add() {
        let mut a = Tally {
            matched: 1,
            missing: 2,
            ..Tally::default()
        };
        a.add(Tally {
            matched: 3,
            extra: 1,
            partial_groups: 1,
            ..Tally::default()
        });
        assert_eq!(
            (a.matched, a.missing, a.extra, a.partial_groups),
            (4, 2, 1, 1)
        );
    }

    fn script(body: &str) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
        ("sh".into(), vec!["-c".into(), body.into()])
    }

    #[test]
    fn a_scan_goes_over_the_pipe_as_one_line_and_one_frame() {
        // Answers `ready`, then one frame for each request line it reads, echoing nothing.
        let (p, a) = script(
            r#"echo ready; while read l; do case "$l" in
              "scan	27	0x00000000000000000000000000000005	512	1") b='1.5	0a	0102
more
'; printf '+%s\n%s' "${#b}" "$b";;
              *) b='bad'; printf -- '-3\nbad';; esac; done"#,
        );
        let mut pipe = ScanPipe::spawn(&p, &a).unwrap();
        let (rows, more) = pipe.scan(27, 5).unwrap();
        assert!(more);
        assert_eq!(rows, vec![held(&[10], 1.5, &[1, 2])]);
    }

    #[test]
    fn a_refusal_is_an_error_and_a_missing_binary_is_too() {
        let (p, a) = script(r#"echo ready; read l; printf -- '-7\nrefused'"#);
        let mut pipe = ScanPipe::spawn(&p, &a).unwrap();
        let e = pipe.scan(27, 1).unwrap_err();
        assert!(e.contains("refused"), "{e}");
        assert!(ScanPipe::spawn("/no/such/geodb-client".as_ref(), &[]).is_err());
        let (p, a) = script("echo not-ready");
        assert!(ScanPipe::spawn(&p, &a).is_err());
    }
}
