# Search Services — Triage & Recovery

What to do when something looks wrong with the `search-indexer` or the search endpoints in `api/`.

For background (architecture, env vars, local setup, env isolation, memory model), see:
- [`search-indexer/README.md`](../../search-indexer/README.md) — the most useful single doc
- [`api/src/services/search/QUERY_ARCHITECTURE.md`](../../api/src/services/search/QUERY_ARCHITECTURE.md) — query/boost reference
- [`search-indexer-deploy/README.md`](../../search-indexer-deploy/README.md) and [`search-indexer-deploy/k8s/jobs/README.md`](../../search-indexer-deploy/k8s/jobs/README.md) — deploy + migration jobs
- [`search-admin/README.md`](../../search-admin/README.md) — index admin CLI
- [`docs/observability.md`](../observability.md) — cross-service observability map

For directly testing search api responses interactively: <https://0xneodev.github.io/geo-search-ui/>.

## First five minutes — where to look

1. **Sentry** — `search-indexer` and `api` projects. Errors and traces (`search_indexer.consume_entities_batch`, `…handle_entity_batch`, `…bulk_operations`).
2. **`indexer.stats` log line** — emitted every 10s, structured. Field reference in `search-indexer/README.md`. This is the fastest way to localize a bottleneck — see triage table below.
3. **Gaia Overview dashboard → OpenSearch section** for OpenSearch health metrics (JVM heap, thread-pool queues, indexing rate, search latency). The OpenSearch instance is provisioned with hundreds of CPUs and lots of RAM, so resource exhaustion on OpenSearch itself is rarely the actual cause — look for query/indexing patterns or upstream issues first.
4. **Kafka consumer lag** — kafka-ui runs in the `kafka` namespace as ClusterIP, port-forward to access:
   ```bash
   kubectl port-forward -n kafka svc/kafka-ui 8080:8080
   ```
   For the search-indexer's consumers, lag on the latest group version should be 0 or near 0. (kafka-ui's Consumers tab is showing 'n/a' for lag, so we have to link each topic's consumers page directly):
   - <http://localhost:8080/ui/clusters/do-managed/all-topics/knowledge.edits/consumer-groups>
   - <http://localhost:8080/ui/clusters/do-managed/all-topics/production.curation.scores/consumer-groups>
   - <http://localhost:8080/ui/clusters/do-managed/all-topics/space.topics/consumer-groups>
   - <http://localhost:8080/ui/clusters/do-managed/all-topics/topology.canonical/consumer-groups>

   No dedicated Grafana panel for indexer lag today — kafka-ui is the source of truth.
5. **Pod state** — `kubectl -n search get pods` (prod) or `-n search-staging` (staging). Look for restarts, OOMKilled, CrashLoopBackOff.

### `indexer.stats` triage

Viewable in the search-indexer pod's stdout logs (no Grafana panel for it):
```bash
kubectl logs -n search -l app=search-indexer --tail=200 -f | grep indexer.stats
```

Stats are emitted every **10 seconds** and rate/delta fields (`events_per_sec`, `ops_per_sec`, `failed_ops`, etc.) are scoped to that 10s window. Zeroes don't necessarily mean broken — they can just mean no events arrived in that window. Sample a few consecutive lines before drawing conclusions, and cross-check the cumulative counters (`events_processed`, `documents_indexed`) which should still be increasing on average.

| What you see | Likely cause |
|---|---|
| Low `events_per_sec`, idle `ops_per_sec` | Kafka-side: consumer lag, slow network, large messages, or upstream not producing |
| High `events_per_sec`, high `avg_bulk_ms` (>200ms) | OpenSearch is the bottleneck — check cluster health, JVM heap, disk |
| `events_per_sec` healthy, `docs_per_sec` low | Most events are score/topic updates, not entity indexes — usually fine during a score backfill |
| `failed_ops > 0` | OpenSearch rejected ops — error log just above has `entity_id` / `operation_type` / message |
| `rss_mb` growing steadily | Leak or unbounded cache; baseline ~565 MiB at typical load |

## Symptom → fix

### Indexer is lagging or stuck

1. Use the `indexer.stats` triage table above to localize the stage.
2. **Kafka stalled** → check `kafka-credentials` secret hasn't rotated; `max.poll.interval.ms` is 1h (PR #506) so transient slowness shouldn't kick the consumer.
3. **Processor stuck on a poison message** → look for repeated errors on the same offset in Sentry. Bumping past it is non-trivial — page Neo before doing anything destructive.

### Pod OOMing or RSS climbing

- Memory baseline + formula: "Memory" section in `search-indexer/README.md`. At typical load (500K canonical spaces, 500K relation-map cache) ~565 MiB; worst case ~1.35 GiB. Prod limit is 4 GiB (PR #512).
- Topology state and the relation-map LRU dominate at scale. Lower `RELATION_MAP_CACHE_SIZE` (default 500K) if memory pressure is real — entries beyond the limit stay on disk in SQLite.
- Large messages (entity batches) can spike memory if the producer sends close to the 20 MB Kafka max. Check upstream for unusually big edits.
- Lowering any of the batch-size envs reduces in-flight memory in the consumer/processor/loader channels. Prod runs at code defaults today (none are set on the deployment):
  - `KAFKA_BATCH_SIZE` (entities) — default `10`
  - `SCORES_BATCH_SIZE` — default `50`
  - `SPACE_TOPICS_BATCH_SIZE` — default `10`
  - `TOPOLOGY_BATCH_SIZE` — default `10`
  - `CHANNEL_BUFFER_SIZE` — default `2`; caps how many batches sit in flight per channel (consumer → processor → loader). Dropping to `1` roughly halves channel memory.

  See the channel-memory table in the indexer README.
- All four consumers (entities, scores, space_topics, topology) feed into a **single shared loader channel** and the same OpenSearch connection. A flood on one stream — e.g. a score backfill — competes with the others for loader bandwidth and OpenSearch capacity, which can show up as edit-indexing lag even though the entities consumer itself is healthy.

### Indexer crash-looping on startup

- Most recent gotcha (PR #618): the `TOPOLOGY_STATE_PATH` env var must be set. In prod it's on the deployment; in tests it has to be passed explicitly.
- Corrupt topology state on the PVC → deleting `/data/topology_state.json` forces a rebuild from Kafka (slow but safe).
- Corrupt relation-map SQLite → deleting `/data/relation_map.sqlite` forces a rebuild. Fast, only affects `DeleteRelation` lookup accuracy until rebuilt.
- OpenSearch unreachable on boot → connection mode defaults to `retry` (15s interval). If a deploy set `fail-fast`, the pod will crash until OpenSearch is up.

### Irrecoverable bug / start from scratch

If a fix has landed and the existing index is unsalvageable, the recovery is: create a fresh empty index at a new version, point the alias at it, then bump the consumer groups so the indexer replays Kafka from offset 0 into the new index.

**1. Find the versions currently in use** (so you pick higher ones):

```bash
# Get OpenSearch index version to point alias to (and what the indexer is configured for)
kubectl -n search get statefulset search-indexer \
  -o jsonpath='{.spec.template.spec.containers[*].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}'

# Versioned indices that actually exist + which one the alias points at
kubectl -n search exec -it <opensearch-pod> -- \
  curl -s 'localhost:9200/_cat/indices/entities_*?v'
kubectl -n search exec -it <opensearch-pod> -- \
  curl -s 'localhost:9200/_cat/aliases/entities?v'

# Get current Kafka consumer group IDs
kubectl -n search get statefulset search-indexer \
  -o jsonpath='{range .spec.template.spec.containers[*].env[?(@.name=="KAFKA_GROUP_EDITS_ID")]}{.value}{"\n"}{end}'
kubectl -n search get statefulset search-indexer \
  -o jsonpath='{range .spec.template.spec.containers[*].env[?(@.name=="KAFKA_GROUP_SCORES_ID")]}{.value}{"\n"}{end}'
```

Pick a target index version and consumer-group suffix **strictly greater** than what's there.

**2. Create the new index + swap the alias** via the search-admin k8s jobs (full CLI reference: [`search-admin/README.md`](../../search-admin/README.md), migration workflow: [`search-indexer-deploy/k8s/jobs/README.md`](../../search-indexer-deploy/k8s/jobs/README.md)):

```bash
cd search-indexer-deploy/k8s/production/jobs
# edit create-index-job.yaml: VERSION=<new>
kubectl apply -f create-index-job.yaml
kubectl logs -n search -f job/opensearch-create-index

# edit update-alias-job.yaml: SOURCE_VERSION=<old>, TARGET_VERSION=<new>
kubectl apply -f update-alias-job.yaml
```

**3. Bump consumer group IDs and `ENTITIES_INDEX_VERSION` on the indexer deployment.** A new consumer group has no committed offsets and `auto.offset.reset=earliest` replays from offset 0 into the freshly-aliased index:

```
ENTITIES_INDEX_VERSION=<m+1>
KAFKA_GROUP_EDITS_ID=search-indexer-group-edits-v<n+1>
KAFKA_GROUP_SCORES_ID=search-indexer-group-scores-v<n+1>
```

Precedent: PR #498. Full reprocessing playbook lives in the indexer README under "Error Recovery → Reprocessing All Events". **For large topics this takes hours** — don't do it casually.

If only a subset of documents is bad, prefer a targeted fix over a full replay.

### Schema change requires a reindex

Use the full-migration k8s Job — no code change required:

```bash
cd search-indexer-deploy/k8s/production/jobs   # or staging/jobs
# edit full-migration-job.yaml: SOURCE_VERSION, TARGET_VERSION
kubectl delete job opensearch-full-migration -n search 2>/dev/null || true
kubectl apply -f full-migration-job.yaml
kubectl logs -n search -f job/opensearch-full-migration
```

Job creates the new index → scales indexer to 0 → reindexes synchronously → swaps the alias → scales indexer up with the new `ENTITIES_INDEX_VERSION`. Full doc: `search-indexer-deploy/k8s/jobs/README.md`.

For one-off ops (create-only, list, delete-old, alias-only), use the per-command jobs in the same directory.

### Index migration partially failed

- Check the job pod logs first — they're synchronous and will say which step failed.
- Re-running the job is safe: `create-index --skip-if-exists` is idempotent, reindex resumes from the OpenSearch task ID, alias swap is idempotent.
- If the alias is left pointing at the wrong index, fix it with `update-alias-job.yaml` **before** scaling the indexer back up.

### Search results look wrong / ranking complaints

- Query construction: `api/src/services/search/opensearch.ts`. Architecture: [`QUERY_ARCHITECTURE.md`](../../api/src/services/search/QUERY_ARCHITECTURE.md).
- **Boost overrides can be passed as `/search` query params** (PR #526) — fastest way to A/B without a deploy.
- Defaults to know:
  - `SCORE_BOOST = 75` (PR #528)
  - unscored entities sit at `0.08` (PR #559)
  - `Comment` type excluded by default (PR #532)
  - `include_non_canonical` defaults to `true` (PR #530)
  - canonical-graph filter applied by default (PR #527)
- "I searched a UUID and got nothing" → the UUID fast path does a `term` lookup on `entity_id`. If the entity isn't in the index at all, that's an indexing problem, not a query problem.

### `additional_space_ids` looks wrong

Recently shipped (PRs #647, #653) — accepts a CSV of up to 10 space UUIDs on GLOBAL-family scopes to widen the eligibility set beyond the canonical graph. **Preston has the most context here, ping him first.**

The filter is a 2-part `bool.should` clause with `minimum_should_match: 1`:
- clause A — `term: { in_canonical_graph: true }` (the canonical-graph anchor, always implicit)
- clause B — `terms: { space_id: [<listed non-root IDs>] }` (the extra spaces)

Result set = canonical-graph entities **OR** entities in any of the listed spaces. The canonical-graph root is implicit, so passing it is a no-op; if the list resolves to only the root, the filter collapses to the bare canonical clause. `include_non_canonical=false` overrides everything (canonical-only wins). Implementation: `buildAdditionalSpacesFilter` in `api/src/services/search/opensearch.ts`.

### Search API erroring or slow

- Check the api service in Sentry first — search resolvers run there.
- OpenSearch unreachable from api → check the api's `OPENSEARCH_URL` and that the OpenSearch pod is up in the `search` namespace.
- Slow queries → cost logger landed in shadow mode (PR #615); check the api's stdout for high-cost search invocations.

