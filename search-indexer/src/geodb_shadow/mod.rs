//! A geo-db twin of the Postgres lookups in [`crate::lookup`], behind a flag that defaults off.
//!
//! With `SEARCH_INDEXER_GEODB_SHADOW=1`, after a SQL lookup succeeds a small sample of its inputs is
//! asked of geo-db too, on a background task, and the two answers are compared. Postgres stays the
//! source of every answer: the shadow never changes what the lookup returns, never delays it, and
//! never fails it. What it produces is a count of agreements and disagreements per lookup
//! ([`SHADOW`], [`SHADOW_MISMATCH`], [`SHADOW_SKIPPED`]) and a `warn!` per disagreeing item, which is
//! the evidence for moving each lookup off Postgres.
//!
//! The comparison runs in geo-db's id space (see [`ids`]): gaia ids are conformed forward the way the
//! feeder conforms them, and gaia Spaces are mapped through the feeder's space map. A gaia Space the
//! map does not hold (or holds as not yet loaded) is skipped and counted, never reported as a gap.
//!
//! Twinned, each through an engine read that exists today:
//! - `spaces_for_entities`: the `holders` read (which Spaces hold any statement of an entity).
//! - `empty_in_space`: the same `holders` read, scoped to the pair's Spaces.
//! - `docs_for_relations`: the `statement` read on the relation's statement id (live relations).
//!
//! Left on SQL only, each with a comment at the lookup naming the read it waits for:
//! `entities_for_spaces`, `kg_applied_block`, and the relation-map bootstrap.

pub mod ids;
pub mod pipe;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hermes_instrumentation::{info, warn};
use tokio::sync::Semaphore;
use uuid::Uuid;

use ids::{conforming_id, SpaceMap, Spaces};
use pipe::{PipeConfig, PipeReader};

/// The flag: `1` or `true` turns the shadow on. Anything else, or unset, leaves it off.
pub const FLAG: &str = "SEARCH_INDEXER_GEODB_SHADOW";
pub const SAMPLE_VAR: &str = "SEARCH_INDEXER_GEODB_SHADOW_SAMPLE";
pub const TIMEOUT_VAR: &str = "SEARCH_INDEXER_GEODB_SHADOW_TIMEOUT_MS";

/// Items compared per lookup call by default, and at most.
const DEFAULT_SAMPLE: usize = 4;
const MAX_SAMPLE: usize = 32;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5000);
const COOLDOWN: Duration = Duration::from_secs(30);
/// Most Spaces one request line may scope (`geodb-client pipe`, as the api's client caps it).
const MAX_LINE_SPACES: usize = 1024;

/// One per sampled item: `lookup`, `result` = `match` | `mismatch` | `failed`.
pub const SHADOW: &str = "search_indexer_geodb_shadow_total";
/// One per disagreeing element: `lookup`, `kind` = `missing_in_geodb` | `extra_in_geodb`.
pub const SHADOW_MISMATCH: &str = "search_indexer_geodb_shadow_mismatch_total";
/// Comparisons not made: `lookup`, `reason` = `busy` | `space_not_loaded` | `space_map_unreadable`.
pub const SHADOW_SKIPPED: &str = "search_indexer_geodb_shadow_skipped_total";

pub const SPACES_FOR_ENTITIES: &str = "spaces_for_entities";
pub const EMPTY_IN_SPACE: &str = "empty_in_space";
pub const DOCS_FOR_RELATIONS: &str = "docs_for_relations";

/// Describe the shadow's series. Call once, after the recorder is installed.
pub fn describe() {
    metrics::describe_counter!(
        SHADOW,
        "Sampled lookup items also asked of geo-db, by lookup and result (match, mismatch, failed)"
    );
    metrics::describe_counter!(
        SHADOW_MISMATCH,
        "Elements where geo-db disagreed with Postgres, by lookup and kind \
         (missing_in_geodb, extra_in_geodb)"
    );
    metrics::describe_counter!(
        SHADOW_SKIPPED,
        "Shadow comparisons not made, by lookup and reason \
         (busy, space_not_loaded, space_map_unreadable)"
    );
}

#[derive(Debug, thiserror::Error)]
pub enum ShadowError {
    #[error("geodb-client did not start: {0}")]
    Spawn(std::io::Error),
    #[error("geodb-client i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("geodb-client closed its output")]
    Closed,
    #[error("geodb-client did not say ready: {0:?}")]
    NotReady(String),
    #[error("geodb-client sent a bad frame header: {0:?}")]
    BadFrame(String),
    #[error("geo-db refused the read: {0}")]
    Refused(String),
    #[error("geo-db read timed out after {0:?}")]
    Timeout(Duration),
    #[error("geodb-client failed recently; cooling down")]
    CoolingDown,
    #[error("request has no Spaces or breaks the line format")]
    BadRequest,
    #[error("space map: {0}")]
    SpaceMap(String),
    #[error("geo-db answered a row this reader does not understand: {0:?}")]
    BadRow(String),
}

/// One geo-db read: a GeoQL document over a scope of geo-db Spaces, answering rows (one a line,
/// tab-separated cells) or a refusal.
#[async_trait]
pub trait GeoDbReader: Send + Sync {
    async fn query(&self, spaces: &[String], doc: &str) -> Result<String, ShadowError>;
}

#[derive(Clone, Debug)]
pub struct ShadowConfig {
    pub pipe: PipeConfig,
    pub space_map: PathBuf,
    pub sample: usize,
}

impl ShadowConfig {
    /// `Ok(None)` when the flag is off. With it on, `GEODB_KEY_PATH`, `GEODB_BINDING_PATH` and
    /// `GEODB_SPACE_MAP_PATH` are required; `GEODB_CLIENT_BIN` defaults to `geodb-client` on `PATH`.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let on = get(FLAG).is_some_and(|v| matches!(v.trim(), "1" | "true"));
        if !on {
            return Ok(None);
        }
        let need = |name: &str| {
            get(name)
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{FLAG} is on but {name} is not set"))
        };
        let key = need("GEODB_KEY_PATH")?;
        let binding = need("GEODB_BINDING_PATH")?;
        let space_map = need("GEODB_SPACE_MAP_PATH")?;
        let program = get("GEODB_CLIENT_BIN")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "geodb-client".into());
        let sample = match get(SAMPLE_VAR) {
            None => DEFAULT_SAMPLE,
            Some(v) => v
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=MAX_SAMPLE).contains(n))
                .ok_or_else(|| format!("{SAMPLE_VAR} must be 1..={MAX_SAMPLE}"))?,
        };
        let timeout = match get(TIMEOUT_VAR) {
            None => DEFAULT_TIMEOUT,
            Some(v) => v
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis)
                .ok_or_else(|| format!("{TIMEOUT_VAR} must be a positive number of ms"))?,
        };
        Ok(Some(Self {
            pipe: PipeConfig {
                program: program.into(),
                args: vec![
                    "--key".into(),
                    key.into(),
                    "--binding".into(),
                    binding.into(),
                    "pipe".into(),
                ],
                timeout,
                cooldown: COOLDOWN,
            },
            space_map: space_map.into(),
            sample,
        }))
    }
}

/// What one comparison found. Item counts feed [`SHADOW`], element counts [`SHADOW_MISMATCH`],
/// `unmapped` feeds [`SHADOW_SKIPPED`] `space_not_loaded`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub matched: u64,
    pub mismatched: u64,
    pub failed: u64,
    pub missing: u64,
    pub extra: u64,
    pub unmapped: u64,
    /// One line per disagreeing or failed item, for the log.
    pub notes: Vec<String>,
}

impl Outcome {
    fn item(&mut self, what: impl std::fmt::Display, missing: usize, extra: usize) {
        if missing == 0 && extra == 0 {
            self.matched += 1;
        } else {
            self.mismatched += 1;
            self.missing += missing as u64;
            self.extra += extra as u64;
            self.notes.push(format!(
                "{what}: {missing} missing_in_geodb, {extra} extra_in_geodb"
            ));
        }
    }

    fn fail(&mut self, what: impl std::fmt::Display, e: &ShadowError) {
        self.failed += 1;
        self.notes.push(format!("{what}: {e}"));
    }
}

/// Count an outcome, and log it when anything disagreed or failed.
pub fn record(lookup: &'static str, o: &Outcome) {
    let total = |result: &'static str, n: u64| {
        if n > 0 {
            metrics::counter!(SHADOW, "lookup" => lookup, "result" => result).increment(n);
        }
    };
    total("match", o.matched);
    total("mismatch", o.mismatched);
    total("failed", o.failed);
    for (kind, n) in [("missing_in_geodb", o.missing), ("extra_in_geodb", o.extra)] {
        if n > 0 {
            metrics::counter!(SHADOW_MISMATCH, "lookup" => lookup, "kind" => kind).increment(n);
        }
    }
    skipped(lookup, "space_not_loaded", o.unmapped);
    if o.mismatched > 0 || o.failed > 0 {
        warn!(
            event = "geodb_shadow_mismatch",
            lookup = lookup,
            matched = o.matched,
            mismatched = o.mismatched,
            failed = o.failed,
            notes = %o.notes.join("; "),
            "geo-db shadow disagreed with Postgres"
        );
    }
}

fn skipped(lookup: &'static str, reason: &'static str, n: u64) {
    if n > 0 {
        metrics::counter!(SHADOW_SKIPPED, "lookup" => lookup, "reason" => reason).increment(n);
    }
}

/// The cells of each row line; blank and `#` (plan, comment) lines are skipped.
fn rows(body: &str) -> impl Iterator<Item = Vec<&str>> {
    body.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').collect())
}

/// Which of `scope` hold at least one active statement of `entity` (geo-db ids). One `holders` read
/// per line of up to [`MAX_LINE_SPACES`] Spaces; each row names a holding Space in its value cell.
async fn holders(
    reader: &dyn GeoDbReader,
    scope: &[String],
    entity: &str,
) -> Result<BTreeSet<String>, ShadowError> {
    let doc = format!(
        ".{{ .version = 1, .select = .{{ .h = .{{ .field = \"entity\", \
         .args = .{{ .holders = true, .id = \"{entity}\" }} }} }} }}"
    );
    let mut held = BTreeSet::new();
    for chunk in scope.chunks(MAX_LINE_SPACES) {
        let body = reader.query(chunk, &doc).await?;
        for cells in rows(&body) {
            let space = cells
                .get(1)
                .filter(|s| s.len() == 32)
                .ok_or_else(|| ShadowError::BadRow(cells.join("\t")))?;
            held.insert((*space).to_string());
        }
    }
    Ok(held)
}

/// `(owner entity, Space)` for each Space where the statement `sid` is active. The owner is the
/// entity cell; the Space is the `<space>:` prefix of the statement cell on a multi-Space line, or the
/// line's only Space.
async fn statement_owners(
    reader: &dyn GeoDbReader,
    scope: &[String],
    sid: &str,
) -> Result<BTreeSet<(String, String)>, ShadowError> {
    let doc = format!(
        ".{{ .version = 1, .select = .{{ .s = .{{ .field = \"entity\", \
         .args = .{{ .statement = \"{sid}\" }} }} }} }}"
    );
    let mut found = BTreeSet::new();
    for chunk in scope.chunks(MAX_LINE_SPACES) {
        let body = reader.query(chunk, &doc).await?;
        for cells in rows(&body) {
            let bad = || ShadowError::BadRow(cells.join("\t"));
            let (Some(stmt), Some(owner)) = (cells.get(2), cells.get(3)) else {
                return Err(bad());
            };
            let space = match stmt.split_once(':') {
                Some((space, _)) => space,
                None if chunk.len() == 1 => chunk[0].as_str(),
                None => return Err(bad()),
            };
            found.insert(((*owner).to_string(), space.to_string()));
        }
    }
    Ok(found)
}

fn diff<T: Ord>(expected: &BTreeSet<T>, got: &BTreeSet<T>) -> (usize, usize) {
    (
        expected.difference(got).count(),
        got.difference(expected).count(),
    )
}

/// Twin of `spaces_for_entities`: for each `(entity, Postgres Spaces)`, the geo-db Spaces holding
/// the entity, over every mapped Space.
///
/// Postgres reads `values` only, while `holders` counts any statement, relations included, so an
/// entity whose only footprint in a Space is an outgoing relation shows as `extra_in_geodb`. That is
/// expected and is why the counter splits by kind.
pub async fn compare_spaces_for_entities(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    sample: &[(Uuid, Vec<Uuid>)],
) -> Outcome {
    let mut o = Outcome::default();
    if spaces.all().is_empty() {
        o.unmapped += sample.iter().map(|(_, s)| s.len() as u64).sum::<u64>();
        return o;
    }
    for (entity, sql_spaces) in sample {
        let mut expected = BTreeSet::new();
        for s in sql_spaces {
            match spaces.geo_of(s) {
                Some(g) => {
                    expected.insert(g.to_string());
                }
                None => o.unmapped += 1,
            }
        }
        let id = conforming_id("entity", entity);
        match holders(reader, spaces.all(), &id).await {
            Ok(got) => {
                let (missing, extra) = diff(&expected, &got);
                o.item(format_args!("entity {entity}"), missing, extra);
            }
            Err(e) => o.fail(format_args!("entity {entity}"), &e),
        }
    }
    o
}

/// Twin of `empty_in_space`: for each `(entity, Space)` pair and whether Postgres found it empty,
/// whether geo-db holds anything of the entity in that Space. Both sides mean the same thing (no
/// statement of the entity at all in the Space), so any disagreement is a real one: Postgres holds
/// but geo-db is empty is `missing_in_geodb`, the reverse `extra_in_geodb`.
///
/// One `holders` read per entity, scoped to that entity's sampled Spaces.
pub async fn compare_empty_in_space(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    sample: &[((Uuid, Uuid), bool)],
) -> Outcome {
    let mut o = Outcome::default();
    let mut by_entity: HashMap<Uuid, Vec<(Uuid, String, bool)>> = HashMap::new();
    let mut order = Vec::new();
    for ((entity, space), sql_empty) in sample {
        let Some(geo) = spaces.geo_of(space) else {
            o.unmapped += 1;
            continue;
        };
        let list = by_entity.entry(*entity).or_insert_with(|| {
            order.push(*entity);
            Vec::new()
        });
        list.push((*space, geo.to_string(), *sql_empty));
    }
    for entity in order {
        let list = &by_entity[&entity];
        let mut scope: Vec<String> = list.iter().map(|(_, g, _)| g.clone()).collect();
        scope.sort();
        scope.dedup();
        let id = conforming_id("entity", &entity);
        match holders(reader, &scope, &id).await {
            Ok(held) => {
                for (space, geo, sql_empty) in list {
                    let geo_holds = held.contains(geo);
                    let (missing, extra) = match (*sql_empty, geo_holds) {
                        (false, false) => (1, 0),
                        (true, true) => (0, 1),
                        _ => (0, 0),
                    };
                    o.item(
                        format_args!("entity {entity} in space {space}"),
                        missing,
                        extra,
                    );
                }
            }
            Err(e) => {
                for (space, _, _) in list {
                    o.fail(format_args!("entity {entity} in space {space}"), &e);
                }
            }
        }
    }
    o
}

/// Twin of the live half of `docs_for_relations`: for each relation and the `(from entity, Space)`
/// documents Postgres resolved it to, the owners of its statement in geo-db, over every mapped Space.
///
/// geo-db's statement read answers active statements only, while Postgres also answers from
/// `relation_versions` once the relation is deleted, so a deleted relation shows as
/// `missing_in_geodb`. The deleted half needs a version-log read (GAP5-VERSION-LOG) to twin.
pub async fn compare_docs_for_relations(
    reader: &dyn GeoDbReader,
    spaces: &Spaces,
    sample: &[(Uuid, Vec<(Uuid, Uuid)>)],
) -> Outcome {
    let mut o = Outcome::default();
    if spaces.all().is_empty() {
        o.unmapped += sample.iter().map(|(_, d)| d.len() as u64).sum::<u64>();
        return o;
    }
    for (relation, docs) in sample {
        let mut expected = BTreeSet::new();
        for (from, space) in docs {
            match spaces.geo_of(space) {
                Some(g) => {
                    expected.insert((conforming_id("entity", from), g.to_string()));
                }
                None => o.unmapped += 1,
            }
        }
        let sid = conforming_id("statement", relation);
        match statement_owners(reader, spaces.all(), &sid).await {
            Ok(got) => {
                let (missing, extra) = diff(&expected, &got);
                o.item(format_args!("relation {relation}"), missing, extra);
            }
            Err(e) => o.fail(format_args!("relation {relation}"), &e),
        }
    }
    o
}

/// The shadow a lookup holds. Comparisons run one at a time on a background task; a lookup that
/// finds one already running counts `busy` and moves on, so the shadow's cost is bounded by one
/// in-flight comparison whatever the indexer's rate.
pub struct GeoDbShadow {
    reader: Arc<dyn GeoDbReader>,
    spaces: SpaceMap,
    sample: usize,
    gate: Arc<Semaphore>,
    cursor: AtomicUsize,
}

impl GeoDbShadow {
    pub fn new(reader: Arc<dyn GeoDbReader>, spaces: SpaceMap, sample: usize) -> Self {
        Self {
            reader,
            spaces,
            sample: sample.clamp(1, MAX_SAMPLE),
            gate: Arc::new(Semaphore::new(1)),
            cursor: AtomicUsize::new(0),
        }
    }

    /// From the environment: `None` when the flag is off, or when it is on but misconfigured (logged
    /// loudly, and the indexer carries on without it: the shadow must never stop the indexer).
    pub fn from_env() -> Option<Arc<Self>> {
        match ShadowConfig::from_vars(|k| std::env::var(k).ok()) {
            Ok(None) => None,
            Ok(Some(cfg)) => {
                info!(
                    sample = cfg.sample,
                    timeout_ms = cfg.pipe.timeout.as_millis() as u64,
                    "geo-db shadow on: sampled lookups are compared with geo-db"
                );
                describe();
                Some(Arc::new(Self::new(
                    Arc::new(PipeReader::new(cfg.pipe)),
                    SpaceMap::from_path(cfg.space_map),
                    cfg.sample,
                )))
            }
            Err(e) => {
                warn!(error = %e, "geo-db shadow is misconfigured; running without it");
                None
            }
        }
    }

    /// Up to `sample` distinct items of `items`, starting at a rotating offset so successive calls
    /// look at different parts of a batch.
    fn pick<T: Clone + PartialEq>(&self, items: &[T]) -> Vec<T> {
        if items.is_empty() {
            return Vec::new();
        }
        let start = self.cursor.fetch_add(self.sample, Ordering::Relaxed) % items.len();
        let mut out: Vec<T> = Vec::with_capacity(self.sample);
        for i in 0..items.len() {
            if out.len() == self.sample {
                break;
            }
            let item = &items[(start + i) % items.len()];
            if !out.contains(item) {
                out.push(item.clone());
            }
        }
        out
    }

    /// Run `compare` on a background task if no other comparison is in flight. Never blocks, never
    /// fails the caller; outside a runtime it does nothing.
    fn launch<F, Fut>(self: &Arc<Self>, lookup: &'static str, compare: F)
    where
        F: FnOnce(Arc<dyn GeoDbReader>, Arc<Spaces>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Outcome> + Send + 'static,
    {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Ok(permit) = self.gate.clone().try_acquire_owned() else {
            skipped(lookup, "busy", 1);
            return;
        };
        let spaces = match self.spaces.current() {
            Ok(s) => s,
            Err(e) => {
                skipped(lookup, "space_map_unreadable", 1);
                warn!(error = %e, lookup = lookup, "geo-db shadow skipped: no space map");
                return;
            }
        };
        let reader = self.reader.clone();
        rt.spawn(async move {
            // No outer timeout: each read is bounded inside the reader, and cancelling a read
            // half-way would leave its reply in the pipe for the next one.
            let o = compare(reader, spaces).await;
            record(lookup, &o);
            drop(permit);
        });
    }

    /// After `spaces_for_entities(entity_ids)` answered `pairs`.
    pub fn observe_spaces_for_entities(
        self: &Arc<Self>,
        entity_ids: &[Uuid],
        pairs: &[(Uuid, Uuid)],
    ) {
        let picked = self.pick(entity_ids);
        if picked.is_empty() {
            return;
        }
        let sample: Vec<(Uuid, Vec<Uuid>)> = picked
            .into_iter()
            .map(|e| {
                let s = pairs
                    .iter()
                    .filter(|(pe, _)| *pe == e)
                    .map(|(_, s)| *s)
                    .collect();
                (e, s)
            })
            .collect();
        self.launch(SPACES_FOR_ENTITIES, move |r, s| async move {
            compare_spaces_for_entities(r.as_ref(), &s, &sample).await
        });
    }

    /// After `empty_in_space(pairs)` answered `empty`.
    pub fn observe_empty_in_space(
        self: &Arc<Self>,
        pairs: &[(Uuid, Uuid)],
        empty: &[(Uuid, Uuid)],
    ) {
        let picked = self.pick(pairs);
        if picked.is_empty() {
            return;
        }
        let sample: Vec<((Uuid, Uuid), bool)> = picked
            .into_iter()
            .map(|p| (p, empty.contains(&p)))
            .collect();
        self.launch(EMPTY_IN_SPACE, move |r, s| async move {
            compare_empty_in_space(r.as_ref(), &s, &sample).await
        });
    }

    /// After `docs_for_relations(relation_ids)` answered `docs`.
    pub fn observe_docs_for_relations(
        self: &Arc<Self>,
        relation_ids: &[Uuid],
        docs: &[(Uuid, Uuid, Uuid)],
    ) {
        let picked = self.pick(relation_ids);
        if picked.is_empty() {
            return;
        }
        let sample: Vec<(Uuid, Vec<(Uuid, Uuid)>)> = picked
            .into_iter()
            .map(|rid| {
                let d = docs
                    .iter()
                    .filter(|(r, _, _)| *r == rid)
                    .map(|(_, e, s)| (*e, *s))
                    .collect();
                (rid, d)
            })
            .collect();
        self.launch(DOCS_FOR_RELATIONS, move |r, s| async move {
            compare_docs_for_relations(r.as_ref(), &s, &sample).await
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::sync::Mutex;

    /// A geo-db from fixtures: which Spaces hold each entity, and each statement's (owner, Space).
    /// Answers in `geodb-client` row format, filtered to the requested scope, and logs each call's
    /// scope size.
    #[derive(Default)]
    struct Fixture {
        holders: HashMap<String, Vec<String>>,
        statements: HashMap<String, Vec<(String, String)>>,
        calls: Mutex<Vec<usize>>,
        fail: bool,
    }

    fn field<'a>(doc: &'a str, name: &str) -> &'a str {
        let at = doc.find(&format!(".{name} = \"")).unwrap() + name.len() + 5;
        &doc[at..at + 32]
    }

    #[async_trait]
    impl GeoDbReader for Fixture {
        async fn query(&self, spaces: &[String], doc: &str) -> Result<String, ShadowError> {
            self.calls.lock().unwrap().push(spaces.len());
            if self.fail {
                return Err(ShadowError::Refused("no".into()));
            }
            let multi = spaces.len() > 1;
            let mut out = String::from("# plan\n");
            if doc.contains(".holders = true") {
                let id = field(doc, "id");
                for s in spaces {
                    if self.holders.get(id).is_some_and(|h| h.contains(s)) {
                        out.push_str(&format!("{}\t{s}\t\t{id}\n", "0".repeat(32)));
                    }
                }
            } else {
                let sid = field(doc, "statement");
                for (owner, s) in self.statements.get(sid).into_iter().flatten() {
                    if spaces.contains(s) {
                        let stmt = if multi {
                            format!("{s}:{sid}")
                        } else {
                            sid.to_string()
                        };
                        out.push_str(&format!(
                            "{}\t{}\t{stmt}\t{owner}\t0\n",
                            "1".repeat(32),
                            "2".repeat(32)
                        ));
                    }
                }
            }
            Ok(out)
        }
    }

    fn u(n: u128) -> Uuid {
        // v4-shaped, so the conforming id is the compact uuid itself.
        uuid::Builder::from_random_bytes(n.to_be_bytes()).into_uuid()
    }

    fn hex(id: &Uuid) -> String {
        id.simple().to_string()
    }

    /// gaia Space n maps to geo Space g(n).
    fn g(n: u128) -> String {
        format!("{:032x}", 0xabc0_0000_0000_0000_0000_0000_0000_0000u128 | n)
    }

    fn map(ns: &[u128]) -> Spaces {
        Spaces::new(ns.iter().map(|n| (hex(&u(1000 + n)), g(*n))).collect())
    }

    fn sp(n: u128) -> Uuid {
        u(1000 + n)
    }

    #[test]
    fn helper_ids_are_v4() {
        let id = u(7);
        assert_eq!(conforming_id("entity", &id), hex(&id));
    }

    #[tokio::test]
    async fn spaces_for_entities_match_missing_extra_and_unmapped() {
        let (e1, e2, e3) = (u(1), u(2), u(3));
        let mut f = Fixture::default();
        f.holders.insert(hex(&e1), vec![g(1), g(2)]);
        f.holders.insert(hex(&e2), vec![g(1)]); // Postgres also has Space 2
        f.holders.insert(hex(&e3), vec![g(1), g(3)]); // geo-db also has Space 3
        let spaces = map(&[1, 2, 3]);
        let sample = vec![
            (e1, vec![sp(1), sp(2)]),
            (e2, vec![sp(1), sp(2), sp(9)]), // Space 9 is not in the map
            (e3, vec![sp(1)]),
        ];
        let o = compare_spaces_for_entities(&f, &spaces, &sample).await;
        assert_eq!((o.matched, o.mismatched, o.failed), (1, 2, 0));
        assert_eq!((o.missing, o.extra, o.unmapped), (1, 1, 1));
        assert_eq!(o.notes.len(), 2);
    }

    #[tokio::test]
    async fn holders_reads_are_cut_at_the_line_cap() {
        let ns: Vec<u128> = (1..=2500).collect();
        let spaces = map(&ns);
        let e = u(1);
        let mut f = Fixture::default();
        f.holders.insert(hex(&e), vec![g(2400)]);
        let o = compare_spaces_for_entities(&f, &spaces, &[(e, vec![sp(2400)])]).await;
        assert_eq!(o.matched, 1);
        assert_eq!(*f.calls.lock().unwrap(), vec![1024, 1024, 452]);
    }

    #[tokio::test]
    async fn empty_in_space_both_directions() {
        let (e1, e2) = (u(1), u(2));
        let mut f = Fixture::default();
        f.holders.insert(hex(&e1), vec![g(1)]);
        f.holders.insert(hex(&e2), vec![g(2)]);
        let spaces = map(&[1, 2, 3]);
        let sample = vec![
            ((e1, sp(1)), false), // both hold
            ((e1, sp(3)), true),  // both empty
            ((e1, sp(2)), false), // Postgres holds, geo-db empty
            ((e2, sp(2)), true),  // Postgres empty, geo-db holds
            ((e2, sp(8)), true),  // unmapped
        ];
        let o = compare_empty_in_space(&f, &spaces, &sample).await;
        assert_eq!((o.matched, o.mismatched), (2, 2));
        assert_eq!((o.missing, o.extra, o.unmapped), (1, 1, 1));
        // One read per entity, scoped to that entity's Spaces only.
        assert_eq!(*f.calls.lock().unwrap(), vec![3, 1]);
    }

    #[tokio::test]
    async fn docs_for_relations_match_deleted_and_differs() {
        let (r1, r2, r3) = (u(11), u(12), u(13));
        let (a, b) = (u(1), u(2));
        let mut f = Fixture::default();
        f.statements.insert(hex(&r1), vec![(hex(&a), g(1))]);
        f.statements.insert(hex(&r3), vec![(hex(&b), g(2))]);
        let spaces = map(&[1, 2]);
        let sample = vec![
            (r1, vec![(a, sp(1))]), // live, agrees
            (r2, vec![(a, sp(1))]), // deleted: only relation_versions has it
            (r3, vec![(a, sp(2))]), // owner differs
        ];
        let o = compare_docs_for_relations(&f, &spaces, &sample).await;
        assert_eq!((o.matched, o.mismatched), (1, 2));
        assert_eq!((o.missing, o.extra), (2, 1));
    }

    #[tokio::test]
    async fn statement_on_a_one_space_line_takes_that_space() {
        let (r, a) = (u(11), u(1));
        let mut f = Fixture::default();
        f.statements.insert(hex(&r), vec![(hex(&a), g(1))]);
        let o = compare_docs_for_relations(&f, &map(&[1]), &[(r, vec![(a, sp(1))])]).await;
        assert_eq!(o.matched, 1);
    }

    #[tokio::test]
    async fn a_failing_reader_counts_failed_items() {
        let f = Fixture {
            fail: true,
            ..Default::default()
        };
        let o = compare_empty_in_space(
            &f,
            &map(&[1]),
            &[((u(1), sp(1)), true), ((u(1), sp(1)), false)],
        )
        .await;
        assert_eq!((o.failed, o.matched, o.mismatched), (2, 0, 0));
    }

    #[test]
    fn config_is_off_unless_flagged_and_needs_its_paths() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(ShadowConfig::from_vars(vars(&[])).unwrap().is_none());
        assert!(ShadowConfig::from_vars(vars(&[(FLAG, "0")]))
            .unwrap()
            .is_none());
        let err = ShadowConfig::from_vars(vars(&[(FLAG, "1")])).unwrap_err();
        assert!(err.contains("GEODB_KEY_PATH"), "{err}");
        let full: &'static [(&str, &str)] = &[
            (FLAG, "true"),
            ("GEODB_KEY_PATH", "/k"),
            ("GEODB_BINDING_PATH", "/b"),
            ("GEODB_SPACE_MAP_PATH", "/m.json"),
        ];
        let c = ShadowConfig::from_vars(vars(full)).unwrap().unwrap();
        assert_eq!(c.sample, DEFAULT_SAMPLE);
        assert_eq!(c.pipe.timeout, DEFAULT_TIMEOUT);
        assert_eq!(c.pipe.program, "geodb-client");
        assert_eq!(c.pipe.args.last().unwrap(), "pipe");
        let bad: &'static [(&str, &str)] = &[
            (FLAG, "1"),
            ("GEODB_KEY_PATH", "/k"),
            ("GEODB_BINDING_PATH", "/b"),
            ("GEODB_SPACE_MAP_PATH", "/m.json"),
            (SAMPLE_VAR, "500"),
        ];
        assert!(ShadowConfig::from_vars(vars(bad)).is_err());
    }

    #[test]
    fn pick_rotates_and_dedups() {
        let s = GeoDbShadow::new(
            Arc::new(Fixture::default()),
            SpaceMap::fixed(HashMap::new()),
            2,
        );
        assert_eq!(s.pick(&[1, 2, 3]), vec![1, 2]);
        assert_eq!(s.pick(&[1, 2, 3]), vec![3, 1]);
        assert_eq!(s.pick(&[5, 5, 5]), vec![5]);
        assert!(s.pick::<u8>(&[]).is_empty());
    }

    #[test]
    fn record_renders_the_three_series() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            describe();
            record(
                EMPTY_IN_SPACE,
                &Outcome {
                    matched: 3,
                    mismatched: 1,
                    failed: 1,
                    missing: 1,
                    extra: 0,
                    unmapped: 2,
                    notes: vec![],
                },
            );
            skipped(EMPTY_IN_SPACE, "busy", 1);
        });
        let out = handle.render();
        for line in [
            "search_indexer_geodb_shadow_total{lookup=\"empty_in_space\",result=\"match\"} 3",
            "search_indexer_geodb_shadow_total{lookup=\"empty_in_space\",result=\"mismatch\"} 1",
            "search_indexer_geodb_shadow_total{lookup=\"empty_in_space\",result=\"failed\"} 1",
            "search_indexer_geodb_shadow_mismatch_total{lookup=\"empty_in_space\",kind=\"missing_in_geodb\"} 1",
            "search_indexer_geodb_shadow_skipped_total{lookup=\"empty_in_space\",reason=\"space_not_loaded\"} 2",
            "search_indexer_geodb_shadow_skipped_total{lookup=\"empty_in_space\",reason=\"busy\"} 1",
        ] {
            assert!(out.contains(line), "missing `{line}` in:\n{out}");
        }
    }

    #[tokio::test]
    async fn observe_runs_in_the_background_and_one_at_a_time() {
        let e = u(1);
        let mut f = Fixture::default();
        f.holders.insert(hex(&e), vec![g(1)]);
        let f = Arc::new(f);
        let by_gaia: HashMap<String, String> = [(hex(&sp(1)), g(1))].into();
        let s = Arc::new(GeoDbShadow::new(f.clone(), SpaceMap::fixed(by_gaia), 4));
        s.observe_spaces_for_entities(&[e], &[(e, sp(1))]);
        // The gate is held until the task finishes; wait for it.
        let _ = s.gate.acquire().await.unwrap();
        assert_eq!(f.calls.lock().unwrap().len(), 1);
    }
}
