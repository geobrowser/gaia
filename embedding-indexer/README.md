# embedding-indexer

Keeps `emb_<slot>` on every in-scope search document equal to the embedding of that document's
current text. One process per embedding slot. It reads the **search view** (the OpenSearch
entities index), never the Kafka edit stream: whatever text a document has is, by definition, the
text to embed, so there is no second derivation of "the entity's name" to drift. Design:
[`docs/tech-designs/semantic-search.md`](../docs/tech-designs/semantic-search.md) (D4, D5, D11).

```
search-indexer ──stamps indexed_at──▶ OpenSearch entities ◀──poll / CAS write── embedding-indexer ──POST /embed──▶ embedding-service
```

## How it works

- **Backfill** (first run, or after the control document is deleted): a full scan in a stable key
  order (`entity_id`, `space_id`), resumable across restarts; covers in-scope documents *and* any
  document still carrying the slot, so out-of-scope leftovers are cleaned.
- **Follow**: every document whose `indexed_at` moved since the checkpoint (minus an overlap
  window), oldest first. Only content writes stamp `indexed_at`, so scoring runs do not trigger
  scans.
- **Per document**: text = `template(name, description)`; if its hash equals `emb_<slot>_src_hash`,
  nothing happens; otherwise the text is embedded (deduplicated within the page and through an
  LRU, so the same claim in several spaces costs one call) and written with a **compare-and-set**
  script that no-ops if the text changed since it was read. Nameless, out-of-type or out-of-space
  documents that still carry slot fields get them removed. Soft-deleted documents are left alone.
- **Never implicit**: at startup the slot's descriptor is read from the index `_meta`, the service's
  `/info` must report the same descriptor hash, and every `/embed` response is checked again. A
  mismatch is fatal; the process exits so the deployment restarts and re-verifies.
- **Errors**: transient (service or OpenSearch 5xx/429, network) → backoff and retry, never skip;
  after three consecutive failures `/health/ready` reports 503. Freshness can stall; correctness
  cannot.

## Run locally

```bash
# OpenSearch from the compose stack, embedding-service on :8090 (see embedding-service/README.md),
# the slot registered on the index (search-admin add-embedding-slot).
ENVIRONMENT=testnet OPENSEARCH_URL=http://localhost:9200 \
EMBEDDING_SERVICE_URL=http://localhost:8090 EMBEDDING_SLOT=79502860cd \
EMBED_INDEX=testnet_entities_v1 EMBED_ONCE=true \
cargo run -p embedding-indexer
```

`EMBED_ONCE=true` finishes the backfill and one follow pass, then exits: the shape of a Kubernetes
Job. Without it the process follows forever, pacing itself with `EMBED_POLL_INTERVAL_MS`.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `OPENSEARCH_URL`, `INDEX_ALIAS`, `ENVIRONMENT` | `http://localhost:9200`, `entities`, — | the index is `<prefix>entities` unless `EMBED_INDEX` names one |
| `EMBED_INDEX` | — | exact index name (bypasses the alias) |
| `EMBED_CONTROL_INDEX` | `<prefix>search_control` | where the per-slot checkpoint lives |
| `EMBEDDING_SERVICE_URL`, `EMBEDDING_SLOT` | — | required |
| `EMBED_POLL_INTERVAL_MS` | 5000 | follow-mode pacing |
| `EMBED_OVERLAP_S` | 30 | follow re-reads documents stamped this long before the checkpoint |
| `EMBED_PAGE_SIZE`, `EMBED_BATCH_SIZE` | 500, 64 | documents per page, texts per service call |
| `EMBED_MAX_DOCS_PER_CYCLE` | 20000 | bound per cycle so follow never starves behind a backfill |
| `EMBED_SCOPE_TYPE_IDS` | all named | comma-separated type entity ids a document must carry |
| `EMBED_SCOPE_SPACE_IDS` | any | comma-separated space ids |
| `EMBED_SKIP_DELETED` | true | leave tombstoned documents alone |
| `EMBED_LRU_SIZE` | 100000 | text-hash → vector cache |
| `EMBED_BACKFILL_BACKEND` | `service` | `extraction_api` is reserved (design D11) |
| `EMBED_ONCE` | false | backfill + one follow pass, then exit |
| `HEALTH_PORT` | 8080 | `/health/live`, `/health/ready` |

## Tests

`cargo test -p embedding-indexer` runs the unit tests (classification, query shapes, scripts). The
integration test runs only with `EMBED_TEST_OPENSEARCH_URL` and `EMBED_TEST_SERVICE_URL` set
(CI sets them; locally the compose OpenSearch and a running service do): it creates a throwaway
index shaped like gaia's, registers the slot, and drives backfill, a name change, an unchanged
pass, a name unset, and a filtered k-NN query.
