//! Ids as geo-db holds them, and the gaia Space -> geo-db Space map.
//!
//! The comparison runs in geo-db's id space: a Postgres id is conformed forward (the same
//! deterministic remap the feeder applies, `geodb-feeder/src/ids.ts` `conformingId`), so no reverse
//! id map is needed. Keep [`conforming_id`] in step with the feeder; the tests pin vectors the
//! feeder's formula produced.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ShadowError;

/// The id geo-db holds for a gaia id of `kind` (`entity`, `statement`, ...): a UUIDv4 passes through
/// as 32 lowercase hex digits; any other id is remapped through SHA-256, as the feeder does.
pub fn conforming_id(kind: &str, id: &Uuid) -> String {
    let compact = id.simple().to_string();
    if is_uuid_v4(&compact) {
        return compact;
    }
    let digest = Sha256::digest(format!("geo:legacy-{kind}:v1\0{compact}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hex::encode(bytes)
}

/// `^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$` on 32 lowercase hex digits.
fn is_uuid_v4(compact: &str) -> bool {
    let b = compact.as_bytes();
    b.len() == 32
        && b.iter().all(u8::is_ascii_hexdigit)
        && b[12] == b'4'
        && matches!(b[16], b'8' | b'9' | b'a' | b'b')
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Parse a space map file: `{"<gaia space id>": {"geo": "<geo-db space id>", "loaded"?: false}}`,
/// the file the feeder maintains and the api reads (`api/src/geodb/client.ts` `parseSpaceMap`).
/// Keys and `geo` must be 32 hex digits (a dashed uuid is compacted); any other entry refuses the
/// whole file, so a damaged map never names a wrong Space. `loaded: false` (founded, history not
/// backfilled) leaves the Space out, so it is skipped rather than reported as a gap.
pub fn parse_space_map(text: &str) -> Result<HashMap<String, String>, String> {
    let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let obj = raw
        .as_object()
        .ok_or_else(|| "space map is not an object".to_string())?;
    let mut map = HashMap::with_capacity(obj.len());
    for (key, entry) in obj {
        let gaia = key.replace('-', "").to_lowercase();
        let geo = entry
            .get("geo")
            .and_then(|g| g.as_str())
            .unwrap_or("")
            .replace('-', "")
            .to_lowercase();
        if !is_hex32(&gaia) || !is_hex32(&geo) {
            return Err(format!("space map entry {key} is not a pair of space ids"));
        }
        if entry.get("loaded").and_then(|l| l.as_bool()) == Some(false) {
            continue;
        }
        map.insert(gaia, geo);
    }
    Ok(map)
}

/// A loaded map, with its geo-db Spaces sorted and distinct (the scope of an every-Space read).
#[derive(Debug, Default)]
pub struct Spaces {
    by_gaia: HashMap<String, String>,
    all: Vec<String>,
}

impl Spaces {
    pub fn new(by_gaia: HashMap<String, String>) -> Self {
        let mut all: Vec<String> = by_gaia.values().cloned().collect();
        all.sort();
        all.dedup();
        Self { by_gaia, all }
    }

    /// The geo-db Space of a gaia Space, or `None` when geo-db does not hold it (yet).
    pub fn geo_of(&self, gaia: &Uuid) -> Option<&str> {
        self.by_gaia
            .get(&gaia.simple().to_string())
            .map(String::as_str)
    }

    /// Every geo-db Space in the map, sorted and distinct.
    pub fn all(&self) -> &[String] {
        &self.all
    }
}

/// How often the file is checked for a newer version (one `stat`).
const REFRESH: Duration = Duration::from_secs(10);

/// A file version: modification time and length.
type Stamp = (SystemTime, u64);

enum Source {
    File {
        path: PathBuf,
        stamp: Option<Stamp>,
        checked: Option<Instant>,
    },
    Fixed,
}

/// The space map, re-read when its file changes. A failed reload keeps the map already held, so a
/// briefly unreadable file never blinds a running pod; only one that never read a good map fails.
pub struct SpaceMap {
    state: Mutex<(Source, Option<Arc<Spaces>>)>,
}

impl SpaceMap {
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            state: Mutex::new((
                Source::File {
                    path,
                    stamp: None,
                    checked: None,
                },
                None,
            )),
        }
    }

    /// A map that never changes (tests).
    pub fn fixed(by_gaia: HashMap<String, String>) -> Self {
        Self {
            state: Mutex::new((Source::Fixed, Some(Arc::new(Spaces::new(by_gaia))))),
        }
    }

    pub fn current(&self) -> Result<Arc<Spaces>, ShadowError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ShadowError::SpaceMap("lock poisoned".into()))?;
        let (source, held) = &mut *guard;
        if let Source::File {
            path,
            stamp,
            checked,
        } = source
        {
            if checked.is_none_or(|t| t.elapsed() >= REFRESH) {
                *checked = Some(Instant::now());
                let reload = || -> Result<Option<(Stamp, Spaces)>, String> {
                    let meta = std::fs::metadata(&*path).map_err(|e| e.to_string())?;
                    let now = (
                        meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        meta.len(),
                    );
                    if held.is_some() && *stamp == Some(now) {
                        return Ok(None);
                    }
                    let text = std::fs::read_to_string(&*path).map_err(|e| e.to_string())?;
                    Ok(Some((now, Spaces::new(parse_space_map(&text)?))))
                };
                match reload() {
                    Ok(Some((now, spaces))) => {
                        *stamp = Some(now);
                        *held = Some(Arc::new(spaces));
                    }
                    Ok(None) => {}
                    Err(e) if held.is_none() => {
                        *checked = None; // retry on the next read
                        return Err(ShadowError::SpaceMap(e));
                    }
                    Err(_) => {}
                }
            }
        }
        held.clone()
            .ok_or_else(|| ShadowError::SpaceMap("no space map".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    // Vectors from geodb-feeder/src/ids.ts `conformingId`, run under node.
    #[test]
    fn conforming_ids_match_the_feeder() {
        let legacy = uuid("00000000-0000-0000-0000-000000000001");
        assert_eq!(
            conforming_id("entity", &legacy),
            "df8ff4dd46e8473f8ca68a78442c9f2b"
        );
        assert_eq!(
            conforming_id("statement", &legacy),
            "4052348e271c41989073c4d7079a5934"
        );
        assert_eq!(
            conforming_id("entity", &uuid("0f1e2d3c4b5a69788796a5b4c3d2e1f0")),
            "8dbfbcdc33784b1fb18c05b1d0793ede"
        );
        let v4 = uuid("6e1f0a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b");
        assert_eq!(
            conforming_id("entity", &v4),
            "6e1f0a2b3c4d4e5f8a9b0c1d2e3f4a5b"
        );
    }

    #[test]
    fn space_map_compacts_skips_unloaded_and_refuses_a_bad_entry() {
        let a = "6e1f0a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
        let text = format!(
            r#"{{"{a}": {{"geo": "{g}"}}, "{b}": {{"geo": "{g2}", "loaded": false}}}}"#,
            g = "a".repeat(32),
            b = "b".repeat(32),
            g2 = "c".repeat(32),
        );
        let map = parse_space_map(&text).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("6e1f0a2b3c4d4e5f8a9b0c1d2e3f4a5b").unwrap(),
            &"a".repeat(32)
        );
        assert!(parse_space_map(r#"{"xyz": {"geo": "aa"}}"#).is_err());
        assert!(parse_space_map("[]").is_err());
    }

    #[test]
    fn file_map_reads_and_keeps_the_last_good_one() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("space-map.json");
        let gaia = "1".repeat(32);
        std::fs::write(
            &p,
            format!(r#"{{"{gaia}": {{"geo": "{}"}}}}"#, "2".repeat(32)),
        )
        .unwrap();
        let map = SpaceMap::from_path(p.clone());
        let spaces = map.current().unwrap();
        assert_eq!(spaces.all(), &["2".repeat(32)]);
        assert_eq!(
            spaces.geo_of(&Uuid::parse_str(&gaia).unwrap()),
            Some("2".repeat(32).as_str())
        );
        let missing = SpaceMap::from_path(dir.path().join("absent.json"));
        assert!(matches!(missing.current(), Err(ShadowError::SpaceMap(_))));
    }
}
