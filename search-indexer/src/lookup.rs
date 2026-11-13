//! Entity-space lookup for resolving doc IDs via Postgres.
//!
//! Queries Postgres tables to resolve:
//! - `entity_id → [space_ids]` for EntityGlobalScore updates (via `values` table)
//! - `space_id → [entity_ids]` for SpaceScore / SpaceTopicEntityId / InCanonicalGraph updates (via `values` table)
//! - `relation_id → (from_entity_id, space_id)` for RemoveRelationById (via `relations`, and
//!   `relation_versions` once kg-indexer has deleted the live row)
//! - which `(entity_id, space_id)` pairs have nothing left in the space, and how far
//!   kg-indexer has got (via `edit_versions`), for retiring emptied documents (GEO-2548)
//!
//! This allows operations to use direct bulk doc ID updates (`_bulk` API)
//! instead of `update_by_query`, reducing indexing time by orders of magnitude.

use hermes_instrumentation::{error, info, warn};
use sqlx::{PgPool, Row};
use std::env;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::errors::IngestError;
use crate::geodb_shadow::serve::{self, AppliedBlockServe, ServeConfig};
use crate::geodb_shadow::GeoDbShadow;
use geodb_cursor::meta_read::MetaBlockShadow;

/// Maximum number of IDs per Postgres lookup batch.
const MAX_BATCH_SIZE: usize = 1000;

/// Default max Postgres connections for the lookup pool.
const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// Entity-space lookup backed by Postgres.
///
/// Queries the `values` and `relations` tables (written by kg-indexer) to resolve
/// doc IDs for direct bulk updates. Results are used to construct OpenSearch doc IDs
/// (`{entity_id}_{space_id}`).
///
/// With `SEARCH_INDEXER_GEODB_SHADOW` on, a sample of some lookups is also asked of geo-db and the
/// answers compared (see [`crate::geodb_shadow`]). Postgres still answers every lookup.
pub struct EntitySpaceLookup {
    pool: PgPool,
    shadow: Option<Arc<GeoDbShadow>>,
    /// Compares the block `kg_applied_block` answers with the one geo-db's feeder records.
    applied_block_shadow: Option<Arc<MetaBlockShadow>>,
    /// Which reads geo-db answers (`SEARCH_INDEXER_GEODB_SERVE`); everything off by default.
    serve: ServeState,
}

/// The serve switches and the state `applied_block` needs. A listed read whose geo-db reader is not
/// configured is switched off at construction, so it can never be half on.
struct ServeState {
    entities_for_spaces: bool,
    applied_block: Option<AppliedBlockServe>,
}

impl ServeState {
    fn off() -> Self {
        Self {
            entities_for_spaces: false,
            applied_block: None,
        }
    }
}

/// Longest the timer's Postgres read of one Space may take.
const TIMER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The `(entity, space)` pairs Postgres holds for one Space, as `entities_for_spaces` reads them,
/// but bounded: `None` when the Space holds more than the shadow's scan cap
/// ([`crate::geodb_shadow::MAX_SCAN_ENTITIES`]), since the comparison would skip it as too large
/// anyway, and the read is cut off after [`TIMER_READ_TIMEOUT`].
async fn bounded_space_pairs(
    pool: &PgPool,
    space: Uuid,
) -> Result<Option<Vec<(Uuid, Uuid)>>, String> {
    let cap = crate::geodb_shadow::MAX_SCAN_ENTITIES as i64;
    let read = sqlx::query_scalar::<_, Uuid>(
        "SELECT DISTINCT entity_id FROM values WHERE space_id = $1 LIMIT $2",
    )
    .bind(space)
    .bind(cap + 1)
    .fetch_all(pool);
    let entities = tokio::time::timeout(TIMER_READ_TIMEOUT, read)
        .await
        .map_err(|_| format!("timed out after {TIMER_READ_TIMEOUT:?}"))?
        .map_err(|e| e.to_string())?;
    if entities.len() as i64 > cap {
        return Ok(None);
    }
    Ok(Some(entities.into_iter().map(|e| (e, space)).collect()))
}

/// The `cursors` row (collection 10) the geodb-feeder writes the chain block it has applied to.
/// See plan 10.22 section 19.
pub const FEEDER_APPLIED_ID: &str = "geodb-feeder-applied";
/// Names this reader in `geodb_meta_block_shadow_total{reader}`.
pub const APPLIED_BLOCK_READER: &str = "search_indexer_retire";

impl EntitySpaceLookup {
    /// Create a new lookup from an existing pool, with no geo-db shadow.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            shadow: None,
            applied_block_shadow: None,
            serve: ServeState::off(),
        }
    }

    /// Create a new lookup whose answers are also compared with geo-db.
    pub fn with_shadow(pool: PgPool, shadow: Arc<GeoDbShadow>) -> Self {
        Self {
            pool,
            shadow: Some(shadow),
            applied_block_shadow: None,
            serve: ServeState::off(),
        }
    }

    /// Also compares `kg_applied_block` with geo-db's record of the feeder's applied block.
    pub fn with_applied_block_shadow(mut self, shadow: Arc<MetaBlockShadow>) -> Self {
        self.applied_block_shadow = Some(shadow);
        self
    }

    /// Answers the reads `cfg` lists from geo-db, falling back to Postgres on any failure. Call after
    /// `with_shadow` / `with_applied_block_shadow`: `entities_for_spaces` needs the shadow's geo-db
    /// reader and `applied_block` the applied-block reader; one that is missing is left off (logged).
    pub fn with_serve(mut self, cfg: &ServeConfig) -> Self {
        if cfg.entities_for_spaces {
            if self.shadow.is_some() {
                self.serve.entities_for_spaces = true;
            } else {
                error!(
                    "{} lists entities_for_spaces but {} is off; serving it from Postgres",
                    serve::SERVE_VAR,
                    crate::geodb_shadow::FLAG
                );
            }
        }
        if cfg.applied_block {
            if self.applied_block_shadow.is_some() {
                self.serve.applied_block = Some(AppliedBlockServe::new(cfg.applied_block_max_age));
            } else {
                error!(
                    "{} lists applied_block but the meta-block reader is off; serving it from Postgres",
                    serve::SERVE_VAR
                );
            }
        }
        if cfg.any() {
            serve::describe();
        }
        self
    }

    /// The feeder's applied block from geo-db, when `applied_block` is served and the row is fresh.
    /// `None` means ask Postgres (the fallback is counted).
    async fn served_applied_block(&self) -> Option<u64> {
        let (state, shadow) = (
            self.serve.applied_block.as_ref()?,
            self.applied_block_shadow.as_ref()?,
        );
        let read = shadow
            .read_block(FEEDER_APPLIED_ID)
            .await
            .map_err(|e| warn!(error = %e, "geo-db applied-block read failed; asking Postgres"));
        match state.decide(read, std::time::Instant::now()) {
            Ok(block) => {
                serve::count_served(serve::READ_APPLIED_BLOCK);
                Some(block)
            }
            Err(reason) => {
                serve::count_fallback(serve::READ_APPLIED_BLOCK, reason);
                None
            }
        }
    }

    /// Counts the geo-db row against the block Postgres answered. A no-op with the shadow off.
    /// Never fails the caller.
    async fn observe_applied_block(&self, pg: Option<u64>) {
        if let Some(shadow) = &self.applied_block_shadow {
            shadow
                .observe(APPLIED_BLOCK_READER, FEEDER_APPLIED_ID, pg)
                .await;
        }
    }

    /// Get a reference to the underlying Postgres pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Connect to Postgres and create a lookup instance.
    ///
    /// Returns `None` if `DATABASE_URL` is not set (graceful degradation).
    /// Logs at `error` level if not set (for Sentry alerting in production).
    pub async fn from_env() -> Option<Self> {
        let database_url = match env::var("DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                error!(
                    "DATABASE_URL not set — score updates will use slow update_by_query path. \
                     Set DATABASE_URL to enable bulk score indexing."
                );
                return None;
            }
        };

        let max_connections: u32 = env::var("DATABASE_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_CONNECTIONS);

        match sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(300))
            .connect(&database_url)
            .await
        {
            Ok(pool) => {
                info!(
                    max_connections = max_connections,
                    "Connected to Postgres for score lookups"
                );
                let shadow = GeoDbShadow::from_env();
                if let Some(shadow) = &shadow {
                    match GeoDbShadow::timer_interval(|k| env::var(k).ok()) {
                        Ok(Some(every)) => {
                            let pool = pool.clone();
                            shadow.spawn_timer(every, move |space| {
                                let pool = pool.clone();
                                async move { bounded_space_pairs(&pool, space).await }
                            });
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!(error = %e, "geo-db shadow timer is misconfigured; running without it")
                        }
                    }
                }
                let lookup = Self {
                    pool,
                    shadow,
                    applied_block_shadow: MetaBlockShadow::from_env().map(Arc::new),
                    serve: ServeState::off(),
                };
                Some(match ServeConfig::from_vars(|k| env::var(k).ok()) {
                    Ok(cfg) if cfg.any() => lookup.with_serve(&cfg),
                    Ok(_) => lookup,
                    Err(e) => {
                        error!(error = %e, "geo-db serve is misconfigured; serving nothing from geo-db");
                        lookup
                    }
                })
            }
            Err(e) => {
                error!(
                    error = %e,
                    "Failed to connect to Postgres for score lookups, \
                     falling back to update_by_query (slow path)"
                );
                None
            }
        }
    }

    /// Given a batch of entity_ids, return all (entity_id, space_id) pairs.
    ///
    /// Batches larger than 1000 are chunked automatically.
    pub async fn spaces_for_entities(
        &self,
        entity_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, IngestError> {
        if entity_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_results = Vec::new();

        for chunk in entity_ids.chunks(MAX_BATCH_SIZE) {
            let start = std::time::Instant::now();
            let chunk_vec: Vec<Uuid> = chunk.to_vec();

            let rows = sqlx::query(
                "SELECT DISTINCT entity_id, space_id FROM values WHERE entity_id = ANY($1)",
            )
            .bind(&chunk_vec)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                IngestError::parse(format!("Postgres lookup failed for entity_ids: {}", e))
            })?;

            let elapsed_ms = start.elapsed().as_millis();
            let pairs_found = rows.len();

            if elapsed_ms > 100 || pairs_found > 5000 {
                info!(
                    entity_ids = chunk.len(),
                    pairs_found = pairs_found,
                    elapsed_ms = elapsed_ms,
                    "Entity→space lookup"
                );
            }

            for row in rows {
                let entity_id: Uuid = row.get("entity_id");
                let space_id: Uuid = row.get("space_id");
                all_results.push((entity_id, space_id));
            }
        }

        if let Some(shadow) = &self.shadow {
            // The twin reads its own sample's rows with the property, to skip the derived ones the
            // feeder never sends; the served answer above is untouched, and a failed read skips it.
            let picked = shadow.sample_entities(entity_ids);
            if !picked.is_empty() {
                match sqlx::query(crate::geodb_shadow::SHADOW_VALUES_SQL)
                    .bind(&picked)
                    .fetch_all(&self.pool)
                    .await
                {
                    Ok(rows) => {
                        let rows: Vec<(Uuid, Uuid, Uuid)> = rows
                            .iter()
                            .map(|r| (r.get("entity_id"), r.get("space_id"), r.get("property_id")))
                            .collect();
                        shadow.observe_spaces_for_entities(picked, &rows);
                    }
                    Err(e) => {
                        warn!(error = %e, "geo-db shadow: values read for spaces_for_entities failed")
                    }
                }
            }
        }
        Ok(all_results)
    }

    /// Given a batch of space_ids, return all (entity_id, space_id) pairs.
    ///
    /// Batches larger than 1000 are chunked automatically.
    ///
    /// With the geo-db shadow on, a sample of the Spaces is also scanned in geo-db
    /// (`space_entities`, one Space at a time) and compared; Postgres still answers.
    pub async fn entities_for_spaces(
        &self,
        space_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, IngestError> {
        if space_ids.is_empty() {
            return Ok(Vec::new());
        }

        if self.serve.entities_for_spaces {
            if let Some(shadow) = &self.shadow {
                match shadow.serve_entities_for_spaces(space_ids).await {
                    Ok(pairs) => {
                        serve::count_served(serve::READ_ENTITIES_FOR_SPACES);
                        return Ok(pairs);
                    }
                    Err(why) => {
                        serve::count_fallback(serve::READ_ENTITIES_FOR_SPACES, why.reason());
                        warn!(reason = why.reason(), detail = ?why, "geo-db could not answer entities_for_spaces; asking Postgres");
                    }
                }
            }
        }

        let mut all_results = Vec::new();

        for chunk in space_ids.chunks(MAX_BATCH_SIZE) {
            let start = std::time::Instant::now();
            let chunk_vec: Vec<Uuid> = chunk.to_vec();

            let rows = sqlx::query(
                "SELECT DISTINCT entity_id, space_id FROM values WHERE space_id = ANY($1)",
            )
            .bind(&chunk_vec)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                IngestError::parse(format!("Postgres lookup failed for space_ids: {}", e))
            })?;

            let elapsed_ms = start.elapsed().as_millis();
            let pairs_found = rows.len();

            if elapsed_ms > 100 || pairs_found > 5000 {
                info!(
                    space_ids = chunk.len(),
                    pairs_found = pairs_found,
                    elapsed_ms = elapsed_ms,
                    "Space→entity lookup"
                );
            }

            for row in rows {
                let entity_id: Uuid = row.get("entity_id");
                let space_id: Uuid = row.get("space_id");
                all_results.push((entity_id, space_id));
            }
        }

        if let Some(shadow) = &self.shadow {
            shadow.observe_entities_for_spaces(space_ids, &all_results);
        }
        Ok(all_results)
    }

    /// Given a batch of relation_ids, return (relation_id, from_entity_id, space_id) tuples:
    /// the document each relation lives on.
    ///
    /// Reads `relations` and also `relation_versions`, because kg-indexer and search-indexer
    /// race on the same edit: by the time a DeleteRelation reaches here the live row is often
    /// gone already, and its version history is the only record of whose relation it was.
    ///
    /// The document belongs to `from_entity_id`. `relations.entity_id` is the relation's own
    /// reified entity, which has no document: resolving through it sent the removal to a doc
    /// that does not exist, where the 404 passes as success, and a relation kg-indexer had
    /// already deleted was skipped outright. The local relation map answers first and gets
    /// this right, which is why no stale relation shows up on live documents (0 of 10,535
    /// sampled on 2026-09-30); this path is the fallback, and the retirement sweep's resolver.
    ///
    /// Batches larger than 1000 are chunked automatically.
    pub async fn docs_for_relations(
        &self,
        relation_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid, Uuid)>, IngestError> {
        if relation_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_results: Vec<(Uuid, Uuid, Uuid)> = Vec::new();
        // The rows that came from the live `relations` table: the only ones geo-db's statement read
        // can answer, so the only ones the shadow compares.
        let mut live_results: Vec<(Uuid, Uuid, Uuid)> = Vec::new();

        for chunk in relation_ids.chunks(MAX_BATCH_SIZE) {
            let start = std::time::Instant::now();
            let chunk_vec: Vec<Uuid> = chunk.to_vec();

            let rows = sqlx::query(DOCS_FOR_RELATIONS_SQL)
                .bind(&chunk_vec)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| {
                    IngestError::parse(format!("Postgres lookup failed for relation_ids: {}", e))
                })?;

            let elapsed_ms = start.elapsed().as_millis();
            let rows_found = rows.len();

            if elapsed_ms > 100 || rows_found > 1000 {
                info!(
                    relation_ids = chunk.len(),
                    rows_found = rows_found,
                    elapsed_ms = elapsed_ms,
                    "Relation→doc lookup"
                );
            }

            for row in rows {
                let relation_id: Uuid = row.get("relation_id");
                let entity_id: Uuid = row.get("from_entity_id");
                let space_id: Uuid = row.get("space_id");
                let live: bool = row.get("live");
                let doc = (relation_id, entity_id, space_id);
                // The same document can come from both tables; answer it once.
                if !all_results.contains(&doc) {
                    all_results.push(doc);
                }
                if live && !live_results.contains(&doc) {
                    live_results.push(doc);
                }
            }
        }

        if let Some(shadow) = &self.shadow {
            shadow.observe_docs_for_relations(relation_ids, &live_results);
        }
        Ok(all_results)
    }

    /// The highest block kg-indexer has committed an edit for.
    ///
    /// kg-indexer applies blocks strictly in ascending order, one transaction per block, and
    /// writes `edit_versions` in that transaction, so every edit at or below this block is
    /// reflected in `values` and `relations`. A backward scan of the
    /// `(block_number, sequence)` unique index, so it costs one index probe.
    ///
    /// Postgres answers. With `GEODB_META_BLOCK_SHADOW` on, the `cursors` row the geodb-feeder
    /// writes (`geodb-feeder-applied`, value `<block>:<per-partition json>`) is also read and
    /// counted against the answer, at most once per sample interval. The feeder row can lag
    /// kg-indexer's (they apply independently), so judge mismatches as a rate. The log's
    /// per-Space version sequence is not chain blocks, which is why the feeder records its own.
    pub async fn kg_applied_block(&self) -> Result<Option<u64>, IngestError> {
        if let Some(block) = self.served_applied_block().await {
            return Ok(Some(block));
        }
        let block: Option<i64> = sqlx::query_scalar(KG_APPLIED_BLOCK_SQL)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                IngestError::parse(format!(
                    "Postgres lookup failed for kg-indexer block: {}",
                    e
                ))
            })?;
        let block = block.and_then(|b| u64::try_from(b).ok());
        self.observe_applied_block(block).await;
        Ok(block)
    }

    /// Of the given `(entity_id, space_id)` pairs, return those with no value and no outgoing
    /// relation in that space: the documents that no longer belong in the index.
    ///
    /// This is the same rule `search-admin reconcile-orphans` uses for `missing_in_space`, and
    /// the one `/search` uses for space membership (`entitySpaceFilterPlugin.ts`). An entity
    /// with no `entities` row has no values either, so it is returned too. Every probe is an
    /// index lookup, so a 1000-pair chunk costs milliseconds.
    pub async fn empty_in_space(
        &self,
        pairs: &[(Uuid, Uuid)],
    ) -> Result<Vec<(Uuid, Uuid)>, IngestError> {
        let mut empty = Vec::new();
        for chunk in pairs.chunks(MAX_BATCH_SIZE) {
            let entity_ids: Vec<Uuid> = chunk.iter().map(|(e, _)| *e).collect();
            let space_ids: Vec<Uuid> = chunk.iter().map(|(_, s)| *s).collect();
            let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(EMPTY_IN_SPACE_SQL)
                .bind(&entity_ids)
                .bind(&space_ids)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| {
                    IngestError::parse(format!("Postgres lookup failed for empty docs: {}", e))
                })?;
            empty.extend(rows);
        }
        if let Some(shadow) = &self.shadow {
            shadow.observe_empty_in_space(pairs, &empty);
        }
        Ok(empty)
    }
}

/// `relation_id → (from_entity_id, space_id)`, live or already deleted. See
/// [`EntitySpaceLookup::docs_for_relations`].
const DOCS_FOR_RELATIONS_SQL: &str = r#"
    SELECT id AS relation_id, from_entity_id, space_id, true AS live
    FROM public.relations WHERE id = ANY($1)
    UNION
    SELECT relation_id, from_entity_id, space_id, false AS live
    FROM public.relation_versions WHERE relation_id = ANY($1)
"#;

/// See [`EntitySpaceLookup::kg_applied_block`].
const KG_APPLIED_BLOCK_SQL: &str = "SELECT max(block_number) FROM public.edit_versions";

/// See [`EntitySpaceLookup::empty_in_space`]. Kept textually close to `check_page` in
/// `search-admin/src/commands/reconcile_orphans.rs` so the two cannot drift apart unnoticed.
const EMPTY_IN_SPACE_SQL: &str = r#"
    SELECT d.entity_id, d.space_id
    FROM unnest($1::uuid[], $2::uuid[]) AS d(entity_id, space_id)
    WHERE NOT EXISTS (SELECT 1 FROM public.values v
                      WHERE v.entity_id = d.entity_id AND v.space_id = d.space_id)
      AND NOT EXISTS (SELECT 1 FROM public.relations r
                      WHERE r.from_entity_id = d.entity_id AND r.space_id = d.space_id)
"#;

#[cfg(test)]
mod applied_block_shadow_tests {
    use super::*;
    use geodb_cursor::derived_read::DerivedReader;
    use geodb_cursor::Config;

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("lazy pool")
    }

    fn fake_shadow(script: &str) -> Arc<MetaBlockShadow> {
        Arc::new(MetaBlockShadow::new(
            DerivedReader::new(Config {
                program: "sh".into(),
                args: vec!["-c".into(), script.into()],
                collection: 0,
                timeout: Duration::from_millis(1500),
                cooldown: Duration::from_secs(60),
            }),
            10,
        ))
    }

    #[test]
    fn the_feeder_row_key_is_the_one_the_feeder_writes() {
        assert_eq!(FEEDER_APPLIED_ID, "geodb-feeder-applied");
    }

    #[tokio::test]
    async fn with_the_shadow_on_the_feeder_row_is_asked_for_in_collection_10() {
        let dir = std::env::temp_dir().join(format!("applied-shadow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("sent");
        let _ = std::fs::remove_file(&log);
        let script = format!(
            "echo ready; while IFS= read -r l; do printf '%s\\n' \"$l\" >> {}; printf '+4\\nnone'; done",
            log.display()
        );
        let lookup =
            EntitySpaceLookup::new(lazy_pool()).with_applied_block_shadow(fake_shadow(&script));
        lookup.observe_applied_block(Some(100)).await;
        let key_hex: String = FEEDER_APPLIED_ID
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("get\t10\thex:{key_hex}\n")
        );
    }

    #[tokio::test]
    async fn with_the_shadow_off_nothing_is_read() {
        let lookup = EntitySpaceLookup::new(lazy_pool());
        assert!(lookup.applied_block_shadow.is_none());
        lookup.observe_applied_block(Some(1)).await;
    }

    #[tokio::test]
    async fn a_dead_geodb_pipe_does_not_fail_the_caller() {
        let lookup =
            EntitySpaceLookup::new(lazy_pool()).with_applied_block_shadow(fake_shadow("exit 1"));
        lookup.observe_applied_block(Some(1)).await;
    }

    /// A geo-db pipe whose applied-block row names `block` (hex of `<block>:c`).
    fn pipe_with_block(block: u64) -> String {
        let hex: String = format!("{block}:c")
            .bytes()
            .map(|b| format!("{b:02x}"))
            .collect();
        let row = format!("{}\\t0\\t{hex}", "0".repeat(32));
        format!(
            "echo ready; while IFS= read -r l; do printf '+{}\\n{row}'; done",
            row.len() - 2
        )
    }

    fn serving(script: &str, max_age_secs: u64) -> EntitySpaceLookup {
        let cfg = ServeConfig {
            applied_block: true,
            applied_block_max_age: Duration::from_secs(max_age_secs),
            ..ServeConfig::default()
        };
        EntitySpaceLookup::new(lazy_pool())
            .with_applied_block_shadow(fake_shadow(script))
            .with_serve(&cfg)
    }

    #[tokio::test]
    async fn applied_block_is_served_from_geodb_once_the_row_is_fresh() {
        let lookup = serving(&pipe_with_block(42), 60);
        // The first read only sets the baseline: Postgres would answer.
        assert_eq!(lookup.served_applied_block().await, None);
        // The row is unchanged but young: served.
        assert_eq!(lookup.served_applied_block().await, Some(42));
    }

    #[tokio::test]
    async fn applied_block_falls_back_when_the_row_is_stale() {
        // A zero-length window is not configurable, so use the smallest (1 s) and wait it out.
        let lookup = serving(&pipe_with_block(7), 1);
        assert_eq!(lookup.served_applied_block().await, None);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(lookup.served_applied_block().await, None);
    }

    #[tokio::test]
    async fn applied_block_falls_back_on_no_row_or_a_dead_pipe() {
        let none = "echo ready; while IFS= read -r l; do printf '+4\\nnone'; done";
        assert_eq!(serving(none, 60).served_applied_block().await, None);
        assert_eq!(serving("exit 1", 60).served_applied_block().await, None);
    }

    #[tokio::test]
    async fn a_listed_read_without_its_geodb_reader_stays_on_postgres() {
        let cfg = ServeConfig {
            applied_block: true,
            entities_for_spaces: true,
            ..ServeConfig::default()
        };
        let lookup = EntitySpaceLookup::new(lazy_pool()).with_serve(&cfg);
        assert!(lookup.serve.applied_block.is_none());
        assert!(!lookup.serve.entities_for_spaces);
        assert_eq!(lookup.served_applied_block().await, None);
    }
}
