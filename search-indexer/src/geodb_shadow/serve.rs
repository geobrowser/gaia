//! Serving reads from geo-db instead of Postgres, behind `SEARCH_INDEXER_GEODB_SERVE`.
//!
//! The compare-only twins in the parent module prove geo-db answers a read the way Postgres does;
//! this promotes two of them to the answer. Each listed read asks geo-db first and, on any failure
//! or any doubt, falls back to Postgres and counts [`FALLBACK`] by read and reason, so a geo-db
//! outage costs the indexer latency, never correctness. Postgres stays the answer for every read
//! not listed. The list is empty (off) by default.
//!
//! Known limit of `entities_for_spaces`: geo-db holds a gaia entity id as the feeder conformed it
//! (see [`super::ids::conforming_id`]). A UUIDv4 passes through unchanged, so the id read back is the
//! gaia id; a non-v4 gaia id was remapped through a hash, which cannot be reversed here, and would
//! come back as a different (valid) id. The shadow's `entities_for_spaces` series is NOT a gate for
//! this: it conforms the Postgres ids forward and compares in geo-db's id space, so it reads zero
//! mismatches whether or not non-v4 ids are present. Do not list `entities_for_spaces` unless every
//! entity id in the Spaces served is a v4 id, or geo-db holds the original (`GEODB_LEGACY_ID_MAP`
//! written by the feeder and backfilled) and this read reverses it; see geodb/RUNBOOK.md,
//! "search-indexer: serving entities_for_spaces from geo-db".

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::ids::Spaces;
use super::{relation_only, space_entities, GeoDbReader, MAX_EXTRA_CHECKS};

/// Comma list of reads to serve from geo-db: `applied_block`, `entities_for_spaces`.
pub const SERVE_VAR: &str = "SEARCH_INDEXER_GEODB_SERVE";
/// How long the feeder's applied block may sit unchanged before `applied_block` stops being served.
pub const APPLIED_MAX_AGE_VAR: &str = "SEARCH_INDEXER_GEODB_SERVE_APPLIED_BLOCK_MAX_AGE_SECS";
const DEFAULT_APPLIED_MAX_AGE: Duration = Duration::from_secs(300);

pub const READ_APPLIED_BLOCK: &str = "applied_block";
pub const READ_ENTITIES_FOR_SPACES: &str = "entities_for_spaces";

/// One per read answered from geo-db: `read`.
pub const SERVED: &str = "search_indexer_geodb_serve_served_total";
/// One per read geo-db did not answer, so Postgres did: `read`, `reason`.
pub const FALLBACK: &str = "search_indexer_geodb_serve_fallback_total";

/// Most Spaces of one served call; a larger batch goes to Postgres (geo-db reads are serial).
pub const MAX_SERVE_SPACES: usize = 16;
/// Most entities of one Space served: past it the relation-only check (two reads a
/// [`MAX_EXTRA_CHECKS`]) costs more than Postgres's one query.
pub const MAX_SERVE_ENTITIES: usize = 512;

/// Which reads are served from geo-db.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeConfig {
    pub applied_block: bool,
    pub entities_for_spaces: bool,
    pub applied_block_max_age: Duration,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            applied_block: false,
            entities_for_spaces: false,
            applied_block_max_age: DEFAULT_APPLIED_MAX_AGE,
        }
    }
}

impl ServeConfig {
    /// Unset or empty: everything off. An unknown entry or a bad age is an error, and the caller
    /// serves nothing (a typo must not silently serve, or silently not).
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut cfg = Self::default();
        if let Some(list) = get(SERVE_VAR) {
            for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                match entry {
                    READ_APPLIED_BLOCK => cfg.applied_block = true,
                    READ_ENTITIES_FOR_SPACES => cfg.entities_for_spaces = true,
                    other => {
                        return Err(format!(
                            "{SERVE_VAR} names an unknown read {other:?} (known: \
                             {READ_APPLIED_BLOCK}, {READ_ENTITIES_FOR_SPACES})"
                        ))
                    }
                }
            }
        }
        if let Some(v) = get(APPLIED_MAX_AGE_VAR) {
            cfg.applied_block_max_age = v
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|s| *s > 0)
                .map(Duration::from_secs)
                .ok_or_else(|| {
                    format!("{APPLIED_MAX_AGE_VAR} must be a positive number of seconds")
                })?;
        }
        Ok(cfg)
    }

    pub fn any(&self) -> bool {
        self.applied_block || self.entities_for_spaces
    }
}

pub fn describe() {
    metrics::describe_counter!(SERVED, "Reads answered from geo-db, by read");
    metrics::describe_counter!(
        FALLBACK,
        "Served reads geo-db did not answer, so Postgres did, by read and reason"
    );
    for read in [READ_APPLIED_BLOCK, READ_ENTITIES_FOR_SPACES] {
        metrics::counter!(SERVED, "read" => read).increment(0);
    }
    for (read, reasons) in [
        (
            READ_APPLIED_BLOCK,
            ["error", "no_row", "stale", "no_baseline"].as_slice(),
        ),
        (
            READ_ENTITIES_FOR_SPACES,
            [
                "error",
                "space_not_loaded",
                "too_large",
                "batch_too_large",
                "bad_id",
            ]
            .as_slice(),
        ),
    ] {
        for reason in reasons {
            metrics::counter!(FALLBACK, "read" => read, "reason" => *reason).increment(0);
        }
    }
}

pub fn count_served(read: &'static str) {
    metrics::counter!(SERVED, "read" => read).increment(1);
}

pub fn count_fallback(read: &'static str, reason: &'static str) {
    metrics::counter!(FALLBACK, "read" => read, "reason" => reason).increment(1);
}

/// Decides whether the feeder's applied block is fresh enough to answer from.
///
/// The row's value (`<block>:<cursor>`) carries no time, so freshness is the age of the last change
/// this process saw: the block has to have moved within `max_age`. The first block seen only sets
/// the baseline (Postgres answers), and a block that stops moving for `max_age` is a stalled
/// feeder (Postgres answers). A block that moves backwards counts as moving.
pub struct AppliedBlockServe {
    max_age: Duration,
    seen: Mutex<Option<(u64, Instant)>>,
}

impl AppliedBlockServe {
    pub fn new(max_age: Duration) -> Self {
        Self {
            max_age,
            seen: Mutex::new(None),
        }
    }

    /// `Ok(block)` to serve, `Err(reason)` to fall back. `read` is geo-db's answer (`Err`: the read
    /// failed), `now` the time of that answer.
    pub fn decide(&self, read: Result<Option<u64>, ()>, now: Instant) -> Result<u64, &'static str> {
        let block = match read {
            Err(()) => return Err("error"),
            Ok(None) => return Err("no_row"),
            Ok(Some(b)) => b,
        };
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        match *seen {
            None => {
                *seen = Some((block, now));
                Err("no_baseline")
            }
            Some((last, _)) if last != block => {
                *seen = Some((block, now));
                Ok(block)
            }
            Some((_, since)) if now.saturating_duration_since(since) <= self.max_age => Ok(block),
            Some(_) => Err("stale"),
        }
    }
}

/// Why `entities_for_spaces` was not answered from geo-db.
#[derive(Debug, PartialEq, Eq)]
pub enum Fallback {
    Error(String),
    SpaceNotLoaded,
    TooLarge,
    BatchTooLarge,
    BadId(String),
}

impl Fallback {
    pub fn reason(&self) -> &'static str {
        match self {
            Fallback::Error(_) => "error",
            Fallback::SpaceNotLoaded => "space_not_loaded",
            Fallback::TooLarge => "too_large",
            Fallback::BatchTooLarge => "batch_too_large",
            Fallback::BadId(_) => "bad_id",
        }
    }
}

/// `entities_for_spaces` from geo-db: for each distinct Space, the entities it holds through
/// `values` (what Postgres reads), as `(entity, space)` pairs. geo-db counts relation-only entities
/// too, so every entity is checked with [`relation_only`] and those are left out. Any Space geo-db
/// cannot answer whole (unmapped, over [`MAX_SERVE_ENTITIES`], a failed read) fails the whole call:
/// a part of an answer is never returned.
pub async fn entities_for_spaces(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    space_ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid)>, Fallback> {
    let distinct: BTreeSet<Uuid> = space_ids.iter().copied().collect();
    if distinct.len() > MAX_SERVE_SPACES {
        return Err(Fallback::BatchTooLarge);
    }
    let mut out = Vec::new();
    for space in distinct {
        let geo = spaces.geo_of(&space).ok_or(Fallback::SpaceNotLoaded)?;
        let held = match space_entities(reader, geo).await {
            Ok(Some(held)) if held.len() <= MAX_SERVE_ENTITIES => held,
            Ok(_) => return Err(Fallback::TooLarge),
            Err(e) => return Err(Fallback::Error(e.to_string())),
        };
        let all: Vec<String> = held.iter().cloned().collect();
        let mut rel = BTreeSet::new();
        for chunk in all.chunks(MAX_EXTRA_CHECKS) {
            rel.extend(
                relation_only(reader, geo, chunk)
                    .await
                    .map_err(|e| Fallback::Error(e.to_string()))?,
            );
        }
        for entity in held.difference(&rel) {
            let id = Uuid::parse_str(entity).map_err(|_| Fallback::BadId(entity.clone()))?;
            out.push((id, space));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geodb_shadow::ids::conforming_id;
    use crate::geodb_shadow::ShadowError;
    use async_trait::async_trait;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn serve_is_off_unless_listed() {
        let cfg = ServeConfig::from_vars(vars(&[])).unwrap();
        assert!(!cfg.any());
        assert!(!ServeConfig::from_vars(vars(&[(SERVE_VAR, " , ")]))
            .unwrap()
            .any());
        let cfg =
            ServeConfig::from_vars(vars(&[(SERVE_VAR, "applied_block, entities_for_spaces")]))
                .unwrap();
        assert!(cfg.applied_block && cfg.entities_for_spaces);
        let cfg = ServeConfig::from_vars(vars(&[(SERVE_VAR, "applied_block")])).unwrap();
        assert!(cfg.applied_block && !cfg.entities_for_spaces);
    }

    #[test]
    fn an_unknown_entry_or_bad_age_refuses_the_config() {
        assert!(ServeConfig::from_vars(vars(&[(SERVE_VAR, "applied_block,nope")])).is_err());
        for bad in ["0", "x", "-1"] {
            assert!(ServeConfig::from_vars(vars(&[(APPLIED_MAX_AGE_VAR, bad)])).is_err());
        }
        let cfg = ServeConfig::from_vars(vars(&[(APPLIED_MAX_AGE_VAR, "30")])).unwrap();
        assert_eq!(cfg.applied_block_max_age, Duration::from_secs(30));
    }

    #[test]
    fn applied_block_is_served_only_once_seen_to_move_and_while_it_keeps_moving() {
        let s = AppliedBlockServe::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(s.decide(Ok(Some(10)), at(0)), Err("no_baseline"));
        // Unchanged inside the bound: fresh.
        assert_eq!(s.decide(Ok(Some(10)), at(30)), Ok(10));
        // Moved: fresh, and the clock restarts.
        assert_eq!(s.decide(Ok(Some(11)), at(50)), Ok(11));
        assert_eq!(s.decide(Ok(Some(11)), at(110)), Ok(11));
        // Stuck past the bound: stale.
        assert_eq!(s.decide(Ok(Some(11)), at(111)), Err("stale"));
        // Moves again: fresh.
        assert_eq!(s.decide(Ok(Some(12)), at(200)), Ok(12));
    }

    #[test]
    fn applied_block_falls_back_on_a_failed_read_or_a_missing_row() {
        let s = AppliedBlockServe::new(Duration::from_secs(60));
        let now = Instant::now();
        assert_eq!(s.decide(Err(()), now), Err("error"));
        assert_eq!(s.decide(Ok(None), now), Err("no_row"));
    }

    /// A geo-db with one `space_entities` scan a Space and per-entity relation-only flags.
    #[derive(Default)]
    struct Fake {
        scans: HashMap<String, Vec<String>>,
        relation_only: BTreeSet<String>,
        fail: bool,
    }

    #[async_trait]
    impl GeoDbReader for Fake {
        async fn query(&self, spaces: &[String], doc: &str) -> Result<String, ShadowError> {
            if self.fail {
                return Err(ShadowError::Refused("no".into()));
            }
            let mut out = String::from("# plan\n");
            if let Some(at) = doc.find(".space_entities = ") {
                let rest = &doc[at + ".space_entities = ".len()..];
                let after = rest.find(".space_entities_after = \"").map(|i| {
                    let s = &rest[i + ".space_entities_after = \"".len()..];
                    s[..32].to_string()
                });
                let all = self.scans.get(&spaces[0]).cloned().unwrap_or_default();
                let mut sorted = all;
                sorted.sort();
                for e in sorted
                    .iter()
                    .filter(|e| after.as_ref().is_none_or(|a| *e > a))
                    .take(512)
                {
                    out.push_str(&format!("x\tx\tx\t{e}\n"));
                }
            } else if let Some(at) = doc.find(".ids = .{ ") {
                let rel_narrow = doc.contains(".relations = true");
                let list = &doc[at + ".ids = .{ ".len()..];
                let list = &list[..list.find('}').unwrap()];
                for id in list.split('"').filter(|t| t.len() == 32) {
                    // A relation-only entity has one statement, a reference; any other has a
                    // reference too and a literal.
                    out.push_str(&format!("x\tx\tx\t{id}\n"));
                    if !rel_narrow && !self.relation_only.contains(id) {
                        out.push_str(&format!("x\tx\tx\t{id}\n"));
                    }
                }
            }
            Ok(out)
        }
    }

    fn v4(n: u8) -> Uuid {
        Uuid::parse_str(&format!("{n:08x}-0000-4000-8000-000000000000")).unwrap()
    }

    fn spaces_with(gaia: &[(Uuid, &str)]) -> Spaces {
        Spaces::new(
            gaia.iter()
                .map(|(g, geo)| (g.simple().to_string(), geo.to_string()))
                .collect(),
        )
    }

    const GEO: &str = "aaaaaaaaaaaa4aaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn serves_the_spaces_entities_without_the_relation_only_ones() {
        let space = v4(1);
        let (a, b, c) = (v4(10), v4(11), v4(12));
        let mut f = Fake::default();
        f.scans.insert(
            GEO.into(),
            [a, b, c]
                .iter()
                .map(|e| conforming_id("entity", e))
                .collect(),
        );
        f.relation_only.insert(conforming_id("entity", &b));
        let got = entities_for_spaces(&f, &spaces_with(&[(space, GEO)]), &[space, space])
            .await
            .unwrap();
        let mut got: Vec<Uuid> = got
            .iter()
            .map(|(e, s)| {
                assert_eq!(*s, space);
                *e
            })
            .collect();
        got.sort();
        assert_eq!(got, vec![a, c]);
    }

    #[tokio::test]
    async fn falls_back_when_geodb_cannot_answer_whole() {
        let space = v4(1);
        let spaces = spaces_with(&[(space, GEO)]);
        // Unmapped Space.
        let f = Fake::default();
        assert_eq!(
            entities_for_spaces(&f, &spaces, &[v4(2)]).await,
            Err(Fallback::SpaceNotLoaded)
        );
        // Failed read.
        let f = Fake {
            fail: true,
            ..Default::default()
        };
        assert!(matches!(
            entities_for_spaces(&f, &spaces, &[space]).await,
            Err(Fallback::Error(_))
        ));
        // Over the serve cap.
        let mut f = Fake::default();
        let many: Vec<String> = (0..=MAX_SERVE_ENTITIES as u32)
            .map(|n| format!("{n:08x}4000800000000000{:08x}", 0))
            .collect();
        f.scans.insert(GEO.into(), many);
        assert_eq!(
            entities_for_spaces(&f, &spaces, &[space]).await,
            Err(Fallback::TooLarge)
        );
        // Too many Spaces in one call.
        let ids: Vec<Uuid> = (0..=MAX_SERVE_SPACES as u8).map(|n| v4(100 + n)).collect();
        assert_eq!(
            entities_for_spaces(&Fake::default(), &spaces, &ids).await,
            Err(Fallback::BatchTooLarge)
        );
    }

    #[test]
    fn describe_registers_every_series() {
        describe();
        assert_eq!(Fallback::TooLarge.reason(), "too_large");
    }
}
