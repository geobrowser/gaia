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

use hermes_instrumentation::{error, info};
use sqlx::{PgPool, Row};
use std::env;
use std::time::Duration;
use uuid::Uuid;

use crate::errors::IngestError;

/// Maximum number of IDs per Postgres lookup batch.
const MAX_BATCH_SIZE: usize = 1000;

/// Default max Postgres connections for the lookup pool.
const DEFAULT_MAX_CONNECTIONS: u32 = 5;

/// Entity-space lookup backed by Postgres.
///
/// Queries the `values` and `relations` tables (written by kg-indexer) to resolve
/// doc IDs for direct bulk updates. Results are used to construct OpenSearch doc IDs
/// (`{entity_id}_{space_id}`).
pub struct EntitySpaceLookup {
    pool: PgPool,
}

impl EntitySpaceLookup {
    /// Create a new lookup from an existing pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
                Some(Self { pool })
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

        Ok(all_results)
    }

    /// Given a batch of space_ids, return all (entity_id, space_id) pairs.
    ///
    /// Batches larger than 1000 are chunked automatically.
    pub async fn entities_for_spaces(
        &self,
        space_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Uuid)>, IngestError> {
        if space_ids.is_empty() {
            return Ok(Vec::new());
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

        let mut all_results = Vec::new();

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
                all_results.push((relation_id, entity_id, space_id));
            }
        }

        Ok(all_results)
    }

    /// The highest block kg-indexer has committed an edit for.
    ///
    /// kg-indexer applies blocks strictly in ascending order, one transaction per block, and
    /// writes `edit_versions` in that transaction, so every edit at or below this block is
    /// reflected in `values` and `relations`. A backward scan of the
    /// `(block_number, sequence)` unique index, so it costs one index probe.
    pub async fn kg_applied_block(&self) -> Result<Option<u64>, IngestError> {
        let block: Option<i64> = sqlx::query_scalar(KG_APPLIED_BLOCK_SQL)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| {
                IngestError::parse(format!(
                    "Postgres lookup failed for kg-indexer block: {}",
                    e
                ))
            })?;
        Ok(block.and_then(|b| u64::try_from(b).ok()))
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
        Ok(empty)
    }
}

/// `relation_id → (from_entity_id, space_id)`, live or already deleted. See
/// [`EntitySpaceLookup::docs_for_relations`].
const DOCS_FOR_RELATIONS_SQL: &str = r#"
    SELECT id AS relation_id, from_entity_id, space_id
    FROM public.relations WHERE id = ANY($1)
    UNION
    SELECT relation_id, from_entity_id, space_id
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
