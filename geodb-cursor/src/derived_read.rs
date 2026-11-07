//! Read side of geo-db's derived class: `get` a row by key and `scan` a page of a group, over the same
//! held `geodb-client derived pipe` child the writers use. The same verbs and answer shapes as the api's
//! reader (`api/src/geodb/derived.ts`).
//!
//! This is for shadows: a caller compares what geo-db holds with what Postgres answered and counts the
//! result. It never serves a request, so every failure is returned for the caller to count and a refusal
//! drops the child (respawned after the cool-down), like the writers. A read needs only an authenticated
//! session, not the writer grant.
//!
//! `get` answers `<group hex>\t<score>\t<value hex>` or `none`. `scan` answers one
//! `<score>\t<key hex>\t<value hex>` a line in score order (descending, then key ascending), and a last
//! line `more` when the group held rows past the page.

use crate::{Config, CursorMirror, MirrorError};

/// Most rows one scan page holds (`wire_derived.page_max`).
pub const SCAN_MAX: usize = 512;
/// Most bytes of one read frame: a full scan page of 48-byte keys and 64-byte values is under 100 KB.
const READ_FRAME_MAX: usize = 1 << 20;

/// One row as `get` answers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Got {
    pub group: u128,
    pub score: f64,
    pub value: Vec<u8>,
}

/// One row of a `scan` page.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRow {
    pub score: f64,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// A scan page: the rows, and whether the group held more.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub rows: Vec<ScanRow>,
    pub more: bool,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The pipe's `get` request line.
pub fn get_line(collection: u32, key: &[u8]) -> String {
    format!("get\t{collection}\thex:{}\n", hex(key))
}

/// The pipe's `scan` request line for the first `first` (1 to [`SCAN_MAX`]) rows of `group`.
pub fn scan_line(collection: u32, group: u128, first: usize) -> String {
    format!("scan\t{collection}\t0x{group:x}\t{first}\t1\n")
}

/// The row a `get` answer names, `None` for `none`. An error on anything else.
pub fn parse_get(body: &str) -> Result<Option<Got>, MirrorError> {
    let bad = || MirrorError::BadFrame(format!("derived get answer: {}", clip(body)));
    let text = body.trim_end_matches('\n');
    if text == "none" {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split('\t').collect();
    let [group, score, value] = parts[..] else {
        return Err(bad());
    };
    if group.len() != 32 {
        return Err(bad());
    }
    let group = u128::from_str_radix(group, 16).map_err(|_| bad())?;
    let score: f64 = score.parse().map_err(|_| bad())?;
    if !score.is_finite() {
        return Err(bad());
    }
    Ok(Some(Got {
        group,
        score,
        value: unhex(value).ok_or_else(bad)?,
    }))
}

/// The page a `scan` answer names. An error on a malformed line.
pub fn parse_scan(body: &str) -> Result<Page, MirrorError> {
    let bad = |l: &str| MirrorError::BadFrame(format!("derived scan row: {}", clip(l)));
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
        let parts: Vec<&str> = line.split('\t').collect();
        let [score, key, value] = parts[..] else {
            return Err(bad(line));
        };
        let score: f64 = score.parse().map_err(|_| bad(line))?;
        if !score.is_finite() {
            return Err(bad(line));
        }
        rows.push(ScanRow {
            score,
            key: unhex(key).ok_or_else(|| bad(line))?,
            value: unhex(value).ok_or_else(|| bad(line))?,
        });
    }
    Ok(Page { rows, more })
}

fn clip(s: &str) -> String {
    s.chars().take(80).collect()
}

/// Holds the `derived pipe` child for reads. Cheap to build; the child starts on first use.
pub struct DerivedReader {
    pipe: CursorMirror,
}

impl DerivedReader {
    pub fn new(cfg: Config) -> Self {
        Self {
            pipe: CursorMirror::new(cfg),
        }
    }

    /// `Some` only when the variable `flag` is `1` or `true` and `GEODB_KEY_PATH` and
    /// `GEODB_BINDING_PATH` are set (the same rule as the writers; `GEODB_CLIENT_BIN` optional).
    pub fn from_vars_for(flag: &str, get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        Config::from_vars_for(flag, get).map(Self::new)
    }

    /// The rows at `keys` of `collection`, in order, `None` where the collection holds none. One
    /// request per key, sent under one hold of the pipe.
    pub async fn get_many(
        &self,
        collection: u32,
        keys: &[Vec<u8>],
    ) -> Result<Vec<Option<Got>>, MirrorError> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let lines: Vec<String> = keys.iter().map(|k| get_line(collection, k)).collect();
        let bodies = self.pipe.exchange(&lines, READ_FRAME_MAX).await?;
        bodies.iter().map(|b| parse_get(b)).collect()
    }

    /// The first `first` rows (clamped to 1..=[`SCAN_MAX`]) of `group` in `collection`.
    pub async fn scan(
        &self,
        collection: u32,
        group: u128,
        first: usize,
    ) -> Result<Page, MirrorError> {
        let line = scan_line(collection, group, first.clamp(1, SCAN_MAX));
        let bodies = self
            .pipe
            .exchange(std::slice::from_ref(&line), READ_FRAME_MAX)
            .await?;
        parse_scan(bodies.first().map(String::as_str).unwrap_or(""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn fake(script: &str) -> DerivedReader {
        DerivedReader::new(Config {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            collection: 0,
            timeout: Duration::from_millis(1500),
            cooldown: Duration::from_secs(60),
        })
    }

    #[test]
    fn request_lines_match_the_api_reader() {
        assert_eq!(get_line(26, &[1, 0xab]), "get\t26\thex:01ab\n");
        assert_eq!(scan_line(11, 0xab, 7), "scan\t11\t0xab\t7\t1\n");
        assert_eq!(scan_line(11, 0, 7), "scan\t11\t0x0\t7\t1\n");
    }

    #[test]
    fn get_answers_parse() {
        assert_eq!(parse_get("none").unwrap(), None);
        let g = parse_get(&format!("{:032x}\t3.5\t0a0b", 9u128))
            .unwrap()
            .unwrap();
        assert_eq!(g.group, 9);
        assert_eq!(g.score, 3.5);
        assert_eq!(g.value, vec![0x0a, 0x0b]);
        assert_eq!(
            parse_get(&format!("{:032x}\t-1\t", 0u128))
                .unwrap()
                .unwrap()
                .value,
            vec![]
        );
        for bad in [
            "",
            "x\t1\t",
            "ab\t1\t00",
            &format!("{:032x}\tnan\t00", 1u128),
            &format!("{:032x}\t1\t0", 1u128),
        ] {
            assert!(parse_get(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn scan_answers_parse_with_and_without_more() {
        let p = parse_scan("2.5\t0102\t0a\n1\t03\t\nmore\n").unwrap();
        assert!(p.more);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].key, vec![1, 2]);
        assert_eq!(p.rows[1].value, Vec::<u8>::new());
        let p = parse_scan("").unwrap();
        assert!(!p.more && p.rows.is_empty());
        assert!(parse_scan("1\tzz\t00\n").is_err());
        assert!(parse_scan("1\t00\n").is_err());
    }

    #[tokio::test]
    async fn get_many_sends_one_line_a_key_and_reads_each_frame() {
        let dir = tempfile_dir("get");
        let log = dir.join("sent");
        let none = "+4\\nnone";
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '{none}'; done",
            log.display()
        );
        let r = fake(&script);
        let got = r.get_many(26, &[vec![1], vec![2]]).await.unwrap();
        assert_eq!(got, vec![None, None]);
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "get\t26\thex:01\nget\t26\thex:02\n"
        );
        assert!(r.get_many(26, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn scan_reads_a_page_and_a_refusal_is_an_error() {
        let body = "1.5\\t01\\t02\\nmore\\n";
        let r = fake(&format!("echo ready; read -r l; printf '+15\\n{body}'"));
        let p = r.scan(11, 5, 10_000).await.unwrap();
        assert!(p.more);
        assert_eq!(p.rows[0].key, vec![1]);
        let r = fake("echo ready; read -r l; printf -- '-9\\nrefused!!'; exit 1");
        assert!(matches!(
            r.scan(11, 5, 1).await,
            Err(MirrorError::Refused(_))
        ));
        assert!(matches!(
            r.scan(11, 5, 1).await,
            Err(MirrorError::CoolingDown)
        ));
    }

    #[test]
    fn the_reader_is_off_unless_flagged_and_pathed() {
        let vars = |flag: &'static str, paths: bool| {
            move |k: &str| match k {
                f if f == flag => Some("1".to_string()),
                "GEODB_KEY_PATH" if paths => Some("/k".into()),
                "GEODB_BINDING_PATH" if paths => Some("/b".into()),
                _ => None,
            }
        };
        assert!(DerivedReader::from_vars_for("F", vars("F", true)).is_some());
        assert!(DerivedReader::from_vars_for("F", vars("F", false)).is_none());
        assert!(DerivedReader::from_vars_for("F", vars("G", true)).is_none());
    }

    fn tempfile_dir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("geodb-read-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
