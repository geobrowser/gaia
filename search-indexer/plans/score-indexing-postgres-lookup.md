# Plan: Postgres Lookup for Score Indexing

## Problem

`EntityGlobalScore` and `SpaceScore` updates use `update_by_query` (scanning the full OpenSearch index per call). With 2.36M entity scores and 727 space scores, this takes ~60+ hours nightly — the search indexer can't finish before the next scoring CronJob runs.

## Solution

Query the existing `values` table (written by kg-indexer) to resolve `entity_id → space_ids` and `space_id → entity_ids`, then use direct bulk updates by doc ID instead of `update_by_query`.

## Architecture

```
Score event (EntityGlobalScore / SpaceScore)
  │
  ▼
Postgres: SELECT DISTINCT entity_id, space_id
          FROM values WHERE entity_id = ANY($1)
  │
  ▼
Resolve doc IDs ({entity_id}_{space_id})
  │
  ▼
POST /entities/_bulk (direct doc ID updates, auto-chunked at 1000 ops)
```

## Lookups

```sql
-- EntityGlobalScore: given up to 1000 entity_ids, get their space_ids
SELECT DISTINCT entity_id, space_id FROM values WHERE entity_id = ANY($1::uuid[])

-- SpaceScore: given up to 1000 space_ids, get their entity_ids
SELECT DISTINCT entity_id, space_id FROM values WHERE space_id = ANY($1::uuid[])
```

Both queries hit the existing `values_entity_space_idx` composite index.

## Changes by Component

### 1. Dependencies

- Add `sqlx` with Postgres feature to `search-indexer/Cargo.toml`
- Add `DATABASE_URL` env var config

### 2. New lookup module (`search-indexer/src/lookup.rs`)

```rust
use sqlx::PgPool;
use uuid::Uuid;

pub struct EntitySpaceLookup {
    pool: PgPool,
}

impl EntitySpaceLookup {
    pub fn new(pool: PgPool) -> Self { ... }

    /// Given a batch of entity_ids (max 1000), return all (entity_id, space_id) pairs.
    pub async fn spaces_for_entities(&self, entity_ids: &[Uuid]) -> Result<Vec<(Uuid, Uuid)>> {
        let rows = sqlx::query(
            "SELECT DISTINCT entity_id, space_id FROM values WHERE entity_id = ANY($1)"
        )
        .bind(entity_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(|r| (r.get("entity_id"), r.get("space_id"))).collect())
    }

    /// Given a batch of space_ids (max 1000), return all (entity_id, space_id) pairs.
    pub async fn entities_for_spaces(&self, space_ids: &[Uuid]) -> Result<Vec<(Uuid, Uuid)>> {
        let rows = sqlx::query(
            "SELECT DISTINCT entity_id, space_id FROM values WHERE space_id = ANY($1)"
        )
        .bind(space_ids)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(|r| (r.get("entity_id"), r.get("space_id"))).collect())
    }
}
```

### 3. Processor changes (`search-indexer/src/processor/mod.rs`)

The processor gets a reference to `EntitySpaceLookup`. When processing score batches:

- **EntityGlobalScore**: Collect up to 1000 entity_ids from the batch → `spaces_for_entities()` → for each `(entity_id, space_id)` pair, emit `ProcessedEvent::UpdateEntityGlobalScoreByDoc { doc_id, score }`
- **SpaceScore**: Collect up to 1000 space_ids from the batch → `entities_for_spaces()` → for each `(entity_id, space_id)` pair, emit `ProcessedEvent::UpdateSpaceScoreByDoc { doc_id, score }`
- **EntitySpaceScore**: No change needed — already uses doc ID

### 4. New ProcessedEvent variants

```rust
pub enum ProcessedEvent {
    // ... existing variants ...

    /// Direct doc-ID update for entity global score (resolved via Postgres lookup)
    UpdateEntityGlobalScoreByDoc {
        doc_id: String,
        score: f64,
    },

    /// Direct doc-ID update for space score (resolved via Postgres lookup)
    UpdateSpaceScoreByDoc {
        doc_id: String,
        score: f64,
    },
}
```

### 5. Loader/Provider changes (`search-indexer-repository/src/opensearch/provider.rs`)

New bulk update handlers:

```rust
EntityOperation::UpdateEntityGlobalScoreByDoc(request) => {
    let body = json!({
        "doc": { "entity_global_score": request.score },
        "doc_as_upsert": true
    });
    bulk_ops.push(BulkOperation::update(request.doc_id, body).into());
}

EntityOperation::UpdateSpaceScoreByDoc(request) => {
    let body = json!({
        "doc": { "space_score": request.score },
        "doc_as_upsert": true
    });
    bulk_ops.push(BulkOperation::update(request.doc_id, body).into());
}
```

These go through the existing bulk pipeline — auto-chunked at `OPENSEARCH_MAX_BULK_SIZE` (default 1000) via the `flush_bulk_if_full!` macro, same as `EntitySpaceScore` already works.

### 6. Fallback to `update_by_query`

If the Postgres lookup returns zero results for an entity/space (race condition with kg-indexer), fall back to the existing `update_by_query` path. Log at `warn` level to track frequency.

### 7. Observability

- **Postgres lookup timing**: Log at `info` level per batch: `"Resolved {n} entity-space pairs from {m} entity_ids in {ms}ms ({cache_misses} from Postgres)"`. This makes lookup latency visible in production logs.
- **Fallback counter**: Track how often the `update_by_query` fallback is hit. If it's frequent, the `values` table may be stale or the scoring CronJob is scoring entities before the kg-indexer has indexed them. Log at `warn`: `"Falling back to update_by_query for entity {id} (not found in values table)"`.
- **Bulk update metrics**: The existing `score_updates` counter and `ops_per_sec` heartbeat will now reflect actual throughput. With bulk updates, `ops_per_sec` should spike during score processing (previously it showed 0 because `update_by_query` wasn't counted as an operation).

### 8. Postgres connection pool config

```rust
let pool = PgPoolOptions::new()
    .max_connections(5)          // Low — only used for score lookups
    .acquire_timeout(Duration::from_secs(10))
    .idle_timeout(Duration::from_secs(300))
    .connect(&database_url)
    .await?;
```

Configurable via env vars:
- `DATABASE_URL` (required)
- `DATABASE_MAX_CONNECTIONS` (default: 5)

The pool is small because lookups are batched (max 1000 IDs per query) and only happen during score processing, not continuously.

### 9. Graceful degradation

If `DATABASE_URL` is not set, log at `error` level at startup so Sentry alerts fire in staging/production:

```
"DATABASE_URL not set — score updates will use slow update_by_query path. Set DATABASE_URL to enable bulk score indexing."
```

If `DATABASE_URL` is set but the connection fails, log at `error` level:

```
"Failed to connect to Postgres for score lookups, falling back to update_by_query (slow path)"
```

In both cases the search-indexer **still starts** and falls back to the existing `update_by_query` path. Postgres is only needed for score lookups — entity indexing is unaffected.

### 10. K8s deployment changes

Production (`search-indexer-deploy/k8s/production/search-indexer.yaml`):
```yaml
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: scoring-service-credentials
      key: DATABASE_URL
- name: DATABASE_MAX_CONNECTIONS
  value: "5"
```

Staging (`search-indexer-deploy/k8s/staging/search-indexer.yaml`):
```yaml
- name: DATABASE_URL
  valueFrom:
    secretKeyRef:
      name: scoring-service-credentials
      key: DATABASE_URL
- name: DATABASE_MAX_CONNECTIONS
  value: "5"
```

No memory changes needed — Postgres lookups don't add to RSS.

### 11. CI/CD

- The existing search-indexer test and deploy workflows don't need changes — the `sqlx` dependency compiles with the rest of the crate.
- The `DATABASE_URL` is optional (graceful degradation), so CI tests run without a Postgres instance.
- Unit tests for the lookup module can use `#[cfg(test)]` with a mock or skip when `DATABASE_URL` is not set.

## Expected Performance

| Phase | Before (measured) | After (projected) |
|---|---|---|
| EntityGlobalScore (2.36M) | 2.36M `update_by_query` calls (~60+ hrs) | 2,360 Postgres lookups + bulk doc ID updates (~2-5 min) |
| SpaceScore (727) | 727 `update_by_query` calls (~30 min, blocked behind entity scores) | 1 Postgres lookup + bulk doc ID updates (~10 sec) |
| EntitySpaceScore (113K) | Already fast (bulk updates) | No change |
| **Total** | **~60+ hours (never finishes)** | **~5 min** |

## Implementation Order

1. Add `sqlx` dependency and `DATABASE_URL` config with connection pool
2. Implement `EntitySpaceLookup` module with batched queries (max 1000 per batch)
3. Wire lookup into processor — resolve doc IDs on EntityGlobalScore and SpaceScore events
4. Add new `ProcessedEvent` variants and corresponding `EntityOperation` types
5. Add bulk update handlers in the OpenSearch provider (auto-chunked at 1000)
6. Add fallback to `update_by_query` when lookup returns empty
7. Add observability: lookup timing logs, fallback counter
8. Graceful degradation if Postgres unavailable
9. Update K8s deployments (production + staging) with `DATABASE_URL` env var
10. Tests

## Future Improvements

- **In-memory LRU cache**: Add an LRU cache (entity_id → space_ids, space_id → entity_ids) in front of Postgres to eliminate network round-trips for hot entries. Updated in real-time from entity Kafka events. Would reduce Postgres lookup overhead from ~25 seconds to near-zero for warm cache.

## Risks

- **Stale data**: If an entity was just created in a space but the kg-indexer hasn't written the value yet, the lookup returns empty and we fall back to `update_by_query`. This is a rare race condition on brand-new entities.
- **Postgres load**: 2,360 batched queries (1000 entity_ids each) during the nightly scoring run. With the existing `values_entity_space_idx` index, each query should take ~5-10ms. Total Postgres load: ~25 seconds. Negligible.
- **Postgres outage**: If Postgres is down during a scoring run, the search-indexer falls back to the slow `update_by_query` path. It won't crash — just slow. An alert on the fallback counter would catch this.
