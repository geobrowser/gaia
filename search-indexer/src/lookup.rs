//! Entity-space lookup for resolving doc IDs via Postgres.
//!
//! Queries Postgres tables to resolve:
//! - `entity_id → [space_ids]` for EntityGlobalScore updates (via `values` table)
//! - `space_id → [entity_ids]` for SpaceScore / SpaceTopicEntityId / InCanonicalGraph updates (via `values` table)
//! - `relation_id → (entity_id, space_id)` for RemoveRelationById (via `relations` table)
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

    /// Given a batch of relation_ids, return (relation_id, entity_id, space_id) tuples.
    ///
    /// Queries the `relations` table to resolve which entity/space each relation belongs to.
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

            let rows = sqlx::query(
                "SELECT id, entity_id, space_id FROM relations WHERE id = ANY($1)",
            )
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
                let relation_id: Uuid = row.get("id");
                let entity_id: Uuid = row.get("entity_id");
                let space_id: Uuid = row.get("space_id");
                all_results.push((relation_id, entity_id, space_id));
            }
        }

        Ok(all_results)
    }
}
