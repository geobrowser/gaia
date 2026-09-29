# Semantic Search — Tech Design (draft, rev 2)

Status: draft for discussion · 2026-09-28 · supersedes the semantic-search half of
[geo-lens](https://github.com/geobrowser/geo-lens) (`docs/design.md` there), which this design
brings inside gaia.

Rev 2 changes (2026-09-28, after reviewing extraction-api): the embedding runtime is a
**gaia-owned Rust service** in the same cluster, not a third-party inference server and not an
external service. Nothing on the query path and nothing in the indexer's steady-state loop
leaves the cluster. extraction-api's Hatchet task plane is kept as an **opt-in backfill backend**
for large one-time loads, bound by the same descriptor rules.

Scope of this draft: **vector similarity search over entities, model-agnostic by construction.**
Reranking, stance/negation handling, query understanding, and arbitrary property filters are
explicitly out of scope (see Non-goals); the API shape leaves room for them.

---

# Background

## What geo-lens proved, and what of it survives

geo-lens is a Neo4j mirror of Geo subgraphs, kept fresh by polling gaia's GraphQL API for
changes, with `exact` / `text` / `vector` query strategies. Two thirds of it is change detection
(per-type `updatedAt` stamps, relation closures, a 3-hourly membership reconcile). Inside gaia the
change stream already exists as Kafka topics and as the OpenSearch search view, so all of that
disappears. What survives:

| geo-lens concept | Becomes in gaia |
|---|---|
| `vector` strategy, per-cache HNSW index | a `knn_vector` field on the existing `entities` search index |
| embedding slot = hash(field, provider) | an **embedding descriptor** with a slot id, stored in the index `_meta` |
| hash-gated embedder (re-embed only when text changed) | the same rule, keyed on the search document's text |
| local ONNX model (bge-small, 384 d) via Python fastembed | the same ONNX artifacts, served by a Rust crate on the ONNX runtime |
| `EmbeddingProvider` protocol with `document` / `query` purpose | the same protocol, in Rust, with the prompt asymmetry carried by the descriptor |
| exactness contract: space-filtered vector queries are exact, never a post-filter | filters *inside* the k-NN clause (OpenSearch efficient filtering) |
| `consistency: cached | bounded | fresh` | not needed; freshness is indexer lag, surfaced as a timestamp |
| per-consumer caches | not needed; a cache was a workaround for API latency, here it is a filter |

The one geo-lens measurement that carries over unchanged: on the `(1 + cos) / 2` score scale a
verbatim duplicate scores 0.99+, a paraphrase ≈ 0.95–0.97, and 0.85 is a usable floor for
bge-small. Floors are **per slot** (see D8) — that number means nothing for another model.

Because Python fastembed and the Rust `fastembed` crate load the *same* ONNX artifacts, a slot
that pins those files reproduces geo-lens's vectors to numerical noise. That turns geo-lens into a
vector-level parity oracle for P1, not just a result-set one.

## The search subsystem today (what this design builds on)

- `search-indexer` consumes `knowledge.edits` directly from Kafka, decodes GRC-20 itself
  (`search-indexer/src/consumer/entities_consumer.rs`), and writes **one document per
  `(entity_id, space_id)`** — doc id `{entity_id}_{space_id}` — into the `entities` alias
  (`{prefix}entities_v{N}`). Postgres is optional for it (score enrichment, relation-map rebuild).
- The document (`search-indexer-shared/src/types/entity_document.rs`) is a flattened search
  card: `name`, `name_raw`, `description`, image fields, `relations` (nested; only the Types,
  Avatar, Cover and Tags relation types), three scores, `space_topic_entity_id`,
  `in_canonical_graph`, `deleted`. **No values, no other relation types.**
- Writes go through `bulk_operations` with the tombstone-dominant script
  (`UPDATE_WITH_TOMBSTONE_CHECK_SCRIPT`): a deleted document ignores merges unless the merge
  itself sets `deleted`. Upserts create a document from the merge payload plus ids.
- `indexed_at` is declared in the mapping as `date` but **no write path sets it today**
  (verified 2026-09-28: the only reference in the search crates is the mapping itself).
- Mapping and analyzers live in one place, `search-indexer-repository/src/opensearch/index_config.rs`.
  Schema changes go through `search-admin` (`create-index`, `reindex`, `update-alias`,
  `full-migration`, `backfill-name-raw` — the last one is the precedent for an additive
  `put_mapping` plus a backfill) and the per-environment k8s jobs in `search-indexer-deploy/k8s/`.
- The API's `/search` route (`api/src/search/index.ts`) composes seven lexical strategies in
  `api/src/services/search/opensearch.ts`, wraps them in `function_score` for score boosts, and
  already supports scope, `space_id`, `type_ids`, `tag_ids`, `exclude_type_ids`,
  `additional_space_ids`, `include_deleted`, `include_non_canonical`.
- OpenSearch is `2.17.1` in both compose files. The official distribution bundles the k-NN
  plugin (HNSW via Lucene or Faiss, efficient filtering, radial `min_score` search) and the
  `hybrid` query with the `normalization-processor` search pipeline. The production cluster's
  flavor is not visible in the repo (`OPENSEARCH_URL` is a secret) — see Open Questions.
- Rust services build on `rust:1.92-bookworm` and ship on `debian:bookworm-slim` (glibc), so the
  ONNX runtime's prebuilt shared library runs unmodified. `axum` (health endpoints), `reqwest`,
  `sha2`, `hex` and `opensearch` are already workspace dependencies; the ONNX runtime is the
  only new native dependency.

## What extraction-api has (reviewed 2026-09-28, `origin/master` b5c92dc)

- A Hatchet task plane: `TaskSpec` per task, `build_task` as the only Hatchet touchpoint, a
  registry, and a facade `POST /tasks` → `{id, status: queued}` / `GET /tasks/{id}` → status and
  result. Enqueue-and-poll only; no wait-for-result path. Per-caller API keys with attribution
  (PR #54). Self-hosted `hatchet-lite` on Railway, project "Geo daily"; the worker is stateless.
- A **dormant** embedding path: an Ollama client for nomic-embed-text (768 d) with an LRU cache,
  `ENABLE_EMBEDDINGS=false` in the env example ("useful for deployment without Ollama"), and a
  pgvector column on the podcast claims table. Hardcoded to one vendor, no purpose / slot /
  revision. geo-lens's `src/embeddings` module is the mature replacement for it.
- Its own precedent already separates the queue from the model server (the Ollama process was
  external). The queue orchestrates; it never was the inference runtime.

---

# General

## Goals

1. `GET /search?mode=semantic` returns the top-k entities by embedding similarity to the query
   text, honoring every existing filter (scope, space, types, tags, canonical, deleted), with a
   per-slot minimum score, in interactive latency.
2. `mode=hybrid` fuses the existing lexical ranking with the semantic one in a single request.
3. **Nothing about an embedding is implicit.** Every stored vector, every query and every
   response names the *slot* that produced it; the slot resolves to an immutable descriptor
   (model artifacts by hash, dimensions, pooling, prompts, text template, truncation, score floor).
4. **Model changes are routine.** A new model is a new slot: backfilled in parallel, compared,
   promoted, retired — never overwritten in place. The procedure is rehearsed before it is needed.
5. **No external dependency on the read path or in steady-state indexing.** The query path and
   the follow loop use only services gaia deploys in its own cluster.
6. No new datastore. No change to the kg-indexer hot path. Minimal change to search-indexer.

## Non-goals (this draft)

- Reranking (cross-encoder or LLM), stance/negation, query understanding — the "beyond
  similarity" track. The `mode` parameter and the response's per-stage fields are designed so
  these can be added as stages without breaking callers.
- Filtering by arbitrary property values or relation types (the document carries neither).
- Hosted embedding providers as *slots* in v1. The provider abstraction allows them; the
  in-cluster rule (Goal 5) means a hosted-model slot would need a proxy on the query path, which
  is a separate decision.
- pgvector / Postgres-side similarity. Adds a large index to a write path the gotchas doc says to
  keep lean, and duplicates filters OpenSearch already has.
- Embedding images or values; embedding user-published `values.embedding` (a GRC-20 data type,
  hex bytes in jsonb) is a separate, protocol-level topic.

## Architecture

```
                 knowledge.edits (Kafka)
                          │
                          ▼
                  ┌───────────────┐   bulk upsert / tombstone script      ┌──────────────────────────┐
                  │ search-indexer│ ─────────────────────────────────────▶ │ OpenSearch               │
                  │ (unchanged,   │   + now stamps indexed_at (D4)         │ {prefix}entities_v{N+1}  │
                  │  +1 line)     │                                        │  index.knn: true         │
                  └───────────────┘                                        │  emb_<slot>  knn_vector  │
                                                                           │  emb_<slot>_src_hash     │
                  ┌─────────────────┐  poll indexed_at ≥ checkpoint        │  emb_<slot>_at           │
                  │ embedding-      │ ◀──────────────────────────────────▶ │  _meta.embedding_slots   │
                  │ indexer (new)   │  CAS write of emb_<slot>             └──────────────────────────┘
                  │                 │                                                    ▲
                  │  EmbedBackend:  │                                                    │ knn / hybrid query
                  │   service ──────┼──────────────┐                                     │ (filters inside knn)
                  │   extraction_api┼──┐           │ POST /embed (documents)   ┌─────────┴──────────┐
                  │   (backfill,    │  │           ▼                           │ api  /search       │
                  │    opt-in)      │  │  ┌─────────────────────┐ POST /embed  │  mode=lexical|     │
                  └─────────────────┘  │  │ embedding-service   │ ◀─────────── │  semantic|hybrid   │
                                       │  │ (new, Rust, axum,   │ GET /info    │                    │
        cluster boundary               │  │  ONNX runtime,      │ ◀─────────── └────────────────────┘
        ───────────────────────────────┼──│  models baked in)   │
                                       │  └─────────────────────┘
                                       ▼
                        extraction-api  POST /tasks  embeddings.embed  (Railway; same ONNX
                        artifacts via Python fastembed; result echoes the provider descriptor)
```

Four moving parts inside the cluster, one optional outside it:

1. **`embedding` crate** (library) — descriptor and slot id, text template, the
   `EmbeddingProvider` trait, the local ONNX provider, model-bundle loading with hash
   verification. Linked by the service and the indexer, so they cannot disagree.
2. **`embedding-service`** (binary) — the one in-cluster inference runtime, HTTP over `axum`.
   Both the indexer (documents) and the API (queries) call it.
3. **`embedding-indexer`** (binary) — keeps `emb_<slot>` on every in-scope document equal to the
   embedding of that document's current text. Reads text from the search view, never from GRC-20.
   Chooses an `EmbedBackend` per mode: `service` always for follow; `service` or
   `extraction_api` for backfill.
4. **Index and API changes** — a new index version with `index.knn: true` and per-slot fields;
   slot descriptors in `_meta`; a search pipeline for hybrid fusion; `mode` / `min_score` / `slot`
   on `/search`.
5. **extraction-api `embeddings.embed`** (external, optional) — an app-agnostic task that embeds
   texts with the same pinned artifacts and echoes its provider descriptor. Used only as a
   backfill backend, only when configured.

## Requirements

| # | Requirement | Where it is met |
|---|---|---|
| R1 | Space-, type-, tag-, canonical-filtered semantic queries are exact within the filter | D7: filter inside the `knn` clause; verified in P0 against brute-force `script_score` |
| R2 | A vector is never served for text other than the text it was computed from | `emb_<slot>_src_hash` + compare-and-set write (§ embedding-indexer) |
| R3 | The model behind any vector or query is recoverable from the response alone | slot id in every response; `_meta.embedding_slots[slot]` is the descriptor |
| R4 | Writer and query side cannot silently run different models | both link the `embedding` crate and verify `GET /info` (artifact hashes) against the descriptor at startup; refuse to run on mismatch |
| R5 | A model rotation needs no downtime and no reindex | additive `put_mapping` per slot; two live slots; default flip is a `_meta` change |
| R6 | search-indexer throughput is unaffected | search-indexer only gains an `indexed_at` stamp; embedding is a separate process with its own backpressure |
| R7 | Semantic search is optional infrastructure, like search itself | if `EMBEDDING_SERVICE_URL` is unset the API mounts only `mode=lexical` |
| R8 | No out-of-cluster dependency on the query path or in the follow loop; an external backend is opt-in for backfill only and every batch it returns is descriptor-verified | D2, D11; `EmbedBackend` contract |

---

# In-Depth

## Embedding descriptor and slots (D3)

The descriptor is an immutable record. Its canonical JSON (sorted keys, no whitespace) is hashed
with SHA-256; the **slot id** is the first 10 hex characters. Change any field → new slot.

```json
{
  "provider":        "onnx-local",
  "model_id":        "BAAI/bge-small-en-v1.5",
  "source":          "hf:qdrant/bge-small-en-v1.5-onnx-q@aa8f8b060edb00e03bfdd08813a2949946c8ba55",
  "artifacts": {
    "model_optimized.onnx":    "sha256:51f1bd0addd6e859e42c2c8021a5e5461385bb676a649f4b269aa445449f2431",
    "tokenizer.json":          "sha256:d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    "config.json":             "sha256:13582bcf2effc85b7bf3d3f5532e686bc1c9ce86bb009d10f0ec33cbe92299dd",
    "special_tokens_map.json": "sha256:5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
    "tokenizer_config.json":   "sha256:0b29c7bfc889e53b36d9dd3e686dd4300f6525110eaa98c76a5dafceb2029f53"
  },
  "dimensions":      384,
  "pooling":         "cls",
  "quantization":    "static",
  "normalize":       true,
  "max_tokens":      512,
  "truncation":      "tail",
  "text_template":   "name_description_v1",
  "document_prompt": "",
  "query_prompt":    "",
  "space_type":      "cosinesimil",
  "score_floor":     0.85
}
```

This is the exact bundle geo-lens runs today (the files Python fastembed 0.8 resolves for
`BAAI/bge-small-en-v1.5`), hashed on 2026-09-29. Two fields deserve a note: `query_prompt` is
**empty** because fastembed applies no instruction to bge-small (geo-lens's adapter documents that
its query and passage paths coincide for this model); the field exists because e5, nomic and
EmbeddingGemma do need one. `quantization` mirrors fastembed's mode for the export (`static` for
this one; `dynamic` forces batch size 1 in fastembed 7.1).

- **Artifacts are pinned by content hash, not by model name.** A model name is ambiguous: the
  Python fastembed default for "bge-small-en-v1.5" is a *quantized* ONNX export from a Qdrant
  mirror, not the BAAI fp32 file, and Hugging Face repositories change under a name. Hashing the
  files is what makes "which model" a fact rather than a label. `model_id` is documentation.
  The ONNX Runtime *version* is reported by `/info` but is **not** part of the slot id: the spike
  measured parity of 1e-6 in cosine between ONNX Runtime 1.28.0 (Rust) and 1.29.0 (Python) on
  these artifacts, so the runtime does not define the space, the artifacts do.
- `text_template` names a function in the `embedding` crate:
  `name_description_v1(name, description) = name + "\n\n" + description` (description omitted when
  absent), NFC-normalized, whitespace-collapsed, truncated to `max_tokens` by the tokenizer. The
  template is part of the descriptor because two deployments of the same weights over different
  text produce incompatible spaces.
- `document_prompt` / `query_prompt` are applied by the `embedding` crate from the descriptor
  according to the request's `purpose`; callers never concatenate prompts themselves.
- `score_floor` is the default `min_score` for the slot. It is measured on the harness, not
  copied from another model.
- **Where it lives:** authored as a `bundle.json` beside the model artifacts (see Model bundles)
  and written into the index by `search-admin add-embedding-slot` as
  `_meta.embedding_slots.<slot>` together with `_meta.embedding_default_slot`. At runtime the
  index is the source of truth for *which slots exist*; the service is the source of truth for
  *which slots are loaded*; the two are compared, never assumed equal.
- **Verification (R4):** at startup the indexer and the API call the service's `GET /info`,
  compare the descriptor for their configured slot field by field (including artifact hashes), and
  exit (indexer) or disable semantic modes (API) on mismatch. The service itself refuses to load a
  bundle whose files do not hash to the descriptor.

## `embedding` crate (library)

New workspace member `embedding/`, no binary. Linked by `embedding-service`, `embedding-indexer`
and `search-admin`.

- `Descriptor` (the record above), `slot_id(&Descriptor)`, `text_template::apply(name, desc)`,
  `content_hash(text)`.
- `trait EmbeddingProvider { fn descriptor(&self) -> &Descriptor; async fn embed(&self, texts:
  &[String], purpose: Purpose) -> Result<Vec<Vec<f32>>>; }` with `Purpose::{Document, Query}`.
- `OnnxLocalProvider`: built on the `fastembed` crate (7.1.0 at the time of the spike, over
  `ort` 2.0.0-rc.13 = ONNX Runtime 1.28.0) using its user-defined-model path
  (`UserDefinedEmbeddingModel::new(onnx_bytes, TokenizerFiles)` + `with_pooling` +
  `with_quantization`; `InitOptionsUserDefined::with_max_length` / `with_intra_threads`), so
  **any** ONNX text-embedding export loads from a bundle — no model allowlist in code. Applies
  prompt, tokenizes, runs the session, pools, normalizes (fastembed normalizes by default;
  verified |v| = 1.000), all per the descriptor. Configurable intra-op threads and max batch.
- `Bundle::load(dir)`: reads `bundle.json`, hashes every artifact, fails on mismatch, returns the
  provider. Bundles are directories baked into the image (`/models/<slot>/`).
- `HttpProvider` (behind a feature flag, not used in v1): the same trait over an HTTP endpoint
  that speaks this service's `/embed` contract, so a future hosted or GPU runtime is a config
  change, not a rewrite.
- Backend-agnostic helpers used by the indexer: batching, in-page dedupe by text hash, LRU.

### Spike results (2026-09-29, Apple M4 10-core laptop, batch 64, claim-length texts)

Scratch crate at the session scratchpad `onnx-spike/` (throwaway; the numbers are what matter).
Same five artifacts on both sides, hashes identical.

| Measurement | Python fastembed 0.8 / ORT 1.29 (geo-lens) | Rust fastembed 7.1 / ORT 1.28 |
|---|---|---|
| Parity on 12 fixed texts + 3 queries (incl. `""`, curly apostrophe, >512-token text) | reference | worst `1 − cos` = **1.05e-6**, max per-component Δ 6e-4; empty text embeds without error |
| Throughput, all 10 cores | 499 texts/s | **660 texts/s** |
| Throughput, `intra_threads = 2` (pod-like) | — | **400 texts/s** |
| `quantization` none vs static on this export | — | identical vectors and speed at batch 64 |
| Model load | — | 0.05 s from bytes |
| Binary | — | 30 MB, ONNX Runtime **statically linked** (no `libonnxruntime` in `otool -L`) |
| Linux, `rust:1.92-bookworm` builder | — | **does not link**: the prebuilt static runtime references `__cxa_call_terminate` and `std::string::_M_replace_cold`, symbols that exist only in libstdc++ from GCC ≥ 13; bookworm ships GCC 12 |
| Linux, `rust:1.92-trixie` builder → `debian:trixie-slim` runtime (GCC 14) | — | links; 33 MB binary; `ldd` shows only system libraries (`libstdc++`, `libgcc_s`, `libm`, `libc`, plus `libssl`/`libcrypto`/`libz`/`libzstd` pulled in by fastembed's default `online` feature); 135 MB image before the model |
| Parity from inside the Linux image (2 CPUs, ORT 1.28) vs Python on the Mac (ORT 1.29) | reference | worst `1 − cos` = **9.4e-7** |
| Throughput inside the Linux image, `--cpus=2`, `intra_threads = 2` (arm64 under Docker's VM) | — | **278 texts/s** |
| x86_64 prebuilt (`ms@1.28.0/x86_64-unknown-linux-gnu`, 101 MB `libonnxruntime.a`, downloaded and inspected with `nm`) | — | references the same GCC ≥ 13 symbols **507 times** — the production architecture has the identical requirement |

Packaging facts from the `ort-sys` build script: under the default `download-binaries` feature it
fetches a prebuilt dist from `cdn.pyke.io` (`pyke:ort-rs/ms@1.28.0/<target>`), verifies its
SHA-256 against a table compiled into the crate, and emits `rustc-link-lib=static=onnxruntime`.
Dists exist for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, both Apple targets and
Windows; **none for musl**, which is fine — gaia ships glibc images. The build stage therefore
needs egress to `cdn.pyke.io` in addition to crates.io; for an air-gapped or fully vendored
build, `ORT_LIB_LOCATION` points the crate at a checked-in static library instead.

**Image base (decided by the spike): the two embedding images build on `rust:1.92-trixie` and
run on `debian:trixie-slim`**, not the bookworm pair the other Rust services use. The prebuilt
static ONNX Runtime 1.28.0 is compiled against libstdc++ from GCC ≥ 13 on both aarch64 and
x86_64, and the binary links `libstdc++.so.6` dynamically, so the *runtime* image needs that ABI
too; bookworm has GCC 12 and fails at link time (and would fail at load time if the binary were
copied in). Same toolchain pin (1.92), different Debian release; nothing else in gaia changes.
Two alternatives were noted and not taken: `ort-load-dynamic` with Microsoft's manylinux
`libonnxruntime.so` (works on bookworm but adds a runtime shared library and a second download
to pin), and building ONNX Runtime from source against bookworm (hours per build).

Crate configuration for the `embedding` crate: `fastembed` with `default-features = false` and
only the ONNX download feature, since bundles are local files — this drops the Hugging Face
client and its OpenSSL/zstd dependencies from the runtime image (the `ldd` line above shows what
the defaults pull in). To confirm in P0's crate scaffold.

Alternative runtime considered: `candle` (pure Rust, no native library) — fewer deployment
moving parts but slower on CPU and more hand-written per-architecture code. `ort` is chosen for
v1 because it runs the identical artifacts geo-lens and Python fastembed use, which is what makes
the parity oracle and the extraction-api backfill backend exact rather than approximate. The
choice is confined to `OnnxLocalProvider` and can change behind the trait.

## Model bundles and images

`models/<slot>/` in the image contains `model.onnx`, `tokenizer.json`, `config.json` (and
`special_tokens_map.json` / `tokenizer_config.json` where the tokenizer needs them) plus
`bundle.json` = the descriptor. Bundles are produced by a script (`scripts/make-bundle.sh
<hf-repo> <revision> <file list>`) that downloads at build time, hashes, and writes
`bundle.json`; the resulting slot id is printed and becomes part of the image tag
(`geo/embedding-service:<slot>[,<slot>]`). A running image therefore *is* its slot list; no
weights are downloaded at runtime and a cold pod serves immediately.

## `embedding-service` (binary)

- `axum` server, in-cluster `ClusterIP` only, no auth (same trust model as the other in-cluster
  services); reachable from the `knowledge` / `knowledge-staging` namespaces.
- Endpoints:
  - `POST /embed` `{ slot, purpose: "document" | "query", texts: [..] }` →
    `{ slot, descriptor_hash, vectors: [[f32; dims]], truncated: [bool] }`.
    Limits: `EMBEDDING_MAX_BATCH` (256) texts and `EMBEDDING_MAX_TEXT_CHARS` (8 000) per text;
    a batch exceeding limits is a 413, never silently truncated (truncation to `max_tokens` is
    the tokenizer's, reported per text).
  - `GET /info` → `{ slots: { <slot>: <descriptor> }, runtime: { onnxruntime, threads } }`.
  - `GET /health/live`, `GET /health/ready` (all configured bundles loaded and hash-verified).
- Loads every bundle listed in `EMBEDDING_SLOTS` (comma-separated slot ids) from `/models/`; a
  request naming an unloaded slot is a 404. During a rotation one deployment serves both slots.
- Concurrency: one ONNX session per slot, `EMBEDDING_INTRA_OP_THREADS` = CPU limit, requests
  queued through a bounded semaphore (`EMBEDDING_MAX_INFLIGHT`, 4) so a backfill cannot starve
  query embedding; a separate, smaller fast lane for `purpose=query` (`EMBEDDING_QUERY_INFLIGHT`,
  2) that is never blocked behind document batches.
- Resources: request 1 CPU / 1 Gi, limit 2 CPU / 2 Gi (bge-small is ~130 MB of weights; a
  768-d base model ~450 MB); HPA 1→4 on CPU. Rust services here are usually StatefulSets with one
  replica; this one is a Deployment because it is stateless and scales horizontally.
- Observability: request/batch/text counters and latency histograms tagged by `slot` and
  `purpose`, Sentry via `hermes-instrumentation`, canonical log line per batch.
- Local dev: a `docker-compose.yml` service under the `services` profile; `cargo run -p
  embedding-service` with `EMBEDDING_MODELS_DIR=./models`.

## Index changes

**One-time, in a new index version `v{N+1}` via `search-admin full-migration`** (`index.knn` is a
static setting and cannot be enabled on an existing index):

```jsonc
"settings": { "index.knn": true, /* existing settings unchanged */ },
"mappings": {
  "_meta": {
    "embedding_slots": { "<slot>": { /* descriptor */ } },
    "embedding_default_slot": "<slot>"
  },
  "properties": {
    /* existing fields unchanged */
    "emb_<slot>": {
      "type": "knn_vector", "dimension": 384,
      "method": { "name": "hnsw", "engine": "lucene", "space_type": "cosinesimil",
                  "parameters": { "m": 16, "ef_construction": 128 } }
    },
    "emb_<slot>_src_hash": { "type": "keyword" },   // sha256 of the embedded text
    "emb_<slot>_at":       { "type": "date" }       // when the vector was written
  }
}
```

**Subsequent slots are additive** (`put_mapping`, no reindex) — the `backfill-name-raw` command is
the precedent. Retiring a slot means removing its three fields, which does need an index version;
retirements are batched into the next scheduled migration.

Engine choice: **Lucene** for v1 — vectors live in the Lucene segments (page cache, no native
off-heap allocation to size), efficient filtering falls back to exact scoring when the filter is
restrictive, `min_score` radial queries are supported, and it is the right choice below ~10 M
vectors. Faiss is the upgrade path if a slot grows past that. Vectors stay in `_source` for v1 so
`reindex` carries them (storage cost accepted; see Sizing).

**Search pipeline** (created by `search-admin ensure-search-pipeline`, idempotent):

```json
PUT _search/pipeline/{prefix}hybrid_minmax
{ "phase_results_processors": [ { "normalization-processor": {
    "normalization": { "technique": "min_max" },
    "combination":   { "technique": "arithmetic_mean", "parameters": { "weights": [0.5, 0.5] } } } } ] }
```

It is applied per request (`?search_pipeline=`) in hybrid mode only; the lexical path is untouched.

### Spike results — k-NN half (2026-09-29, gaia's compose OpenSearch 2.17.1, official image)

Run against the local stack with a 4-dimensional toy index so every expected result could be
computed by hand; the same checks are repeated at corpus scale in P1.

| Check | Result |
|---|---|
| Plugins in the image | `opensearch-knn`, `opensearch-ml`, `opensearch-neural-search` present; nothing to install |
| Index with `index.knn: true`, `knn_vector` (lucene / hnsw / cosinesimil), `_meta.embedding_slots` | accepted |
| `knn` with both `k` and `min_score` | **rejected**: "requires exactly one of k, distance or score" → semantic mode uses the radial form |
| `k` + filter inside the clause vs brute-force `script_score` on the filtered set | identical order and scores; a filter selecting a single document still returns it (exact fallback) |
| radial `min_score` + filter | correct on the toy index, but at corpus scale recall is 0.82 with no `ef_search` accepted — **rejected for semantic mode** (see the corpus-scale report) |
| `k` with a larger `size` | `k` caps the hits (k=2, size=5 → 2 hits) |
| filter wrapped *outside* the clause (`bool.filter` around `knn`) | **loses hits**: the clause picked its k globally, then the filter removed them — the post-filter failure D7 forbids, reproduced |
| `hybrid` query + `normalization-processor` pipeline | works; fused scores in `[0, 1]` |
| `hybrid` with `from > 0` | **rejected** verbatim: "In the current OpenSearch version pagination is not supported with hybrid query" → v1 returns 400 for `offset > 0` in hybrid mode |
| `put_mapping` adding `emb_s2` and updating `_meta` on a live index | accepted; `_meta` shows both slots afterwards (D9's additive rotation step, verified) |

**Corpus scale (step 0, same day):** the design's mapping was loaded with 321,408 real claim
documents and their vectors; results, storage breakdown and the query-shape decision are in
[`docs/benchmarks/semantic-search-poc.md`](../benchmarks/semantic-search-poc.md). Headlines:
filtered k-NN ≥ 0.99 recall at 6–9 ms p95 with `ef_search` 256; tag-filtered queries exact at 4–6
ms; hybrid 23–30 ms; the single-segment index is 2.4 GB of which raw vectors are 471 MB and the
HNSW graph 17 MB; store-level parity with geo-lens 20/20.

Local-stack note: on an Apple M4 with Docker Desktop 24 the 2.17.1 image's JDK 21.0.4 aborts
with `SIGILL` at startup (it misdetects SVE under virtualization). Start the container with
`_JAVA_OPTIONS=-XX:UseSVE=0` (a compose override; `JAVA_TOOL_OPTIONS` is deliberately ignored by
OpenSearch's launcher). Worth a line in `docs/gotchas.md` when the feature lands.

## search-indexer: the one change (D4)

Stamp `indexed_at` (UTC now) on **every** operation that writes `name`, `description` or
`deleted`, and on `unset_document_properties`. Sites: `build_update_doc` in
`search-indexer-repository/src/opensearch/provider.rs`, the equivalent in `bulk.rs`, and the unset
path. This fixes a latent gap (the field is mapped but never written) and gives the embedding
indexer its cursor. The search-indexer runs as a single-replica StatefulSet, so one wall clock
stamps every document; the poller still uses an overlap window (below) so clock jitter or a
future second replica cannot lose updates.

## `embedding-indexer` (binary)

**Purpose.** Maintain the invariant: for every in-scope document,
`emb_<slot>` = embed(text_template(doc.name, doc.description)) and
`emb_<slot>_src_hash` = sha256(that text). Out-of-scope documents carry no slot fields.

**Why it reads the view and not the log (D5).** The vector must correspond to the text the
document *has*, and that text is the product of ~1,200 lines of GRC-20 derivation
(create/update/unset/delete/restore, cross-space name resolution) in the search consumer.
Re-deriving it would create a second definition of "the entity's name" that could drift.
Polling the view has precedent in gaia (kg-indexer's tally worker polls its own queue table)
and removes ordering races entirely: whatever text is in the document is, by definition, the
text to embed. A Kafka trigger is the upgrade path if sub-second freshness is ever required
(Alternatives).

**Loop.** One task per configured slot:

```
checkpoint := load(control doc)            // none on first run
loop every EMBED_POLL_INTERVAL_MS:
  if checkpoint is none:                    // backfill: full scan, resumable; backend = EMBED_BACKFILL_BACKEND
      scan = match_all, sort [_doc], PIT + search_after, resume key persisted per page
  else:                                     // follow: backend = service, always
      scan = range indexed_at ≥ checkpoint − EMBED_OVERLAP_S, sort [indexed_at, _id], search_after
  for each page (EMBED_PAGE_SIZE docs, _source: [name, description, deleted, relations, space_id, emb_<slot>_src_hash]):
      partition into: out_of_scope | unchanged (hash equal) | stale
      out_of_scope with slot fields present → CAS-remove slot fields
      stale → dedupe by text hash within the page (same claim in 3 spaces = 1 embed call),
              consult a bounded LRU(text_hash → vector),
              backend.embed(texts, Document) in batches of EMBED_BATCH_SIZE,
              verify the returned descriptor_hash == slot's, else fail the batch loudly,
              CAS-write per document
      persist page cursor; on full-scan completion set checkpoint := scan start time
```

**`EmbedBackend` (D11).** A small trait in the indexer, two implementations:

| Backend | Transport | Used for | Notes |
|---|---|---|---|
| `service` (default) | in-cluster HTTP to `embedding-service` | follow **and** backfill | the only backend allowed in follow mode; the indexer's readiness depends on it |
| `extraction_api` (opt-in) | `POST /tasks {type: "embeddings.embed"}` + poll `GET /tasks/{id}` with `X-API-Key` | backfill only, when `EMBED_BACKFILL_BACKEND=extraction_api` | off-cluster capacity and Hatchet's rate limiting / visibility for large one-time loads; up to `EXTRACTION_API_MAX_INFLIGHT` runs in flight; every result must echo a provider descriptor whose artifact hashes, dimensions, pooling, normalize, prompts and `max_tokens` equal the slot's, otherwise the run is rejected and counted; on `EXTRACTION_API_MAX_FAILURES` consecutive failures the backfill falls back to `service` and alerts, it never stalls |

Both backends are pure functions of `(texts, purpose)`; a duplicate run (the facade's
`idempotency_key` is accepted but not yet wired) costs compute, never correctness. The text
template and the prompts are applied identically for both because both are described by the
same descriptor; the external backend receives the descriptor in the task input and refuses a
descriptor it cannot honor.

**Scope policy** (env; all optional): `EMBED_SCOPE_TYPE_IDS` (allowlist matched against the
document's nested `relations` where `relation_type = Types`; empty = every named document),
`EMBED_SCOPE_SPACE_IDS`, `EMBED_REQUIRE_NAME=true`, `EMBED_SKIP_DELETED=true`. Scope is evaluated
server-side in the scan query where possible (nested type filter, `exists: name`) so the poller
does not page through millions of nameless ids.

**Compare-and-set write** (Painless, one script, via `bulk_operations`):

```painless
if (ctx._source.containsKey('deleted') && ctx._source.deleted == true) { ctx.op = 'noop'; return; }
def n = ctx._source.containsKey('name') ? ctx._source.name : null;
def d = ctx._source.containsKey('description') ? ctx._source.description : null;
if (!Objects.equals(n, params.name) || !Objects.equals(d, params.description)) { ctx.op = 'noop'; return; }
ctx._source[params.vec_field]  = params.vector;
ctx._source[params.hash_field] = params.src_hash;
ctx._source[params.at_field]   = params.now;
```

The guard compares the *text*, not a hash the search-indexer would have to compute, so
search-indexer needs no knowledge of embeddings. If the text changed between read and write the
write is a no-op and the document is picked up again on the next poll (its `indexed_at` moved).
Tombstoned documents keep their last vector; they are already excluded by the `deleted` filter,
and on restore the text is unchanged so the vector is still valid.

**Error policy** (mirrors ranking-indexer): transient failures (backend 5xx/timeout, OpenSearch
429/5xx) retry with exponential backoff and bounded attempts, then the loop pauses and the health
endpoint reports degraded; nothing is skipped. Poison inputs cannot occur — the input is the view.
A service outage therefore stalls freshness, never correctness; the staleness bound is
`poll interval + outage`, and `emb_<slot>_at` makes it observable per document.

**Throughput controls.** `EMBED_BATCH_SIZE` (64 for `service`, 256 for `extraction_api`),
`EMBED_CONCURRENCY` (2 in-flight batches), `EMBED_MAX_DOCS_PER_CYCLE` (bounds a cycle so follow
mode never starves), `EMBED_RATE_LIMIT_TPS` (optional ceiling). Backfill and follow share one
code path; a backfill is just a first run with a possibly different backend.

**State.** A single control document per slot in `{prefix}search_control` (checkpoint, page
cursor, backend in use, last cycle stats). No Postgres, no PVC.

**Observability.** Canonical logs per cycle (`embedding_indexer.cycle_start` / `cycle_end`:
scanned, unchanged, embedded, removed, noop-CAS, backend, descriptor mismatches, backend p50/p95,
OpenSearch p95), metrics tagged by `slot` and `backend`, Sentry via `hermes-instrumentation`,
`HEALTH_PORT` with `/health/live`, `/health/ready` (service `/info` verified, OpenSearch reachable).

**Deployment.** `embedding-indexer/` crate (workspace member, `Dockerfile` copied from
`ranking-indexer`, `k8s/{staging,production,v2}/` Deployment, 1 replica, 100m/256Mi request,
no volume). CI: `embedding-indexer-check.yml` + `embedding-indexer-tests.yml`, and
`dockerfile-bins.yml` already checks every crate ships its binaries.

## API changes

`SearchQuery` (`api/src/services/search/types.ts`) gains:

```ts
mode?: "lexical" | "semantic" | "hybrid"   // default "lexical" (unchanged behavior)
min_score?: number                          // semantic/hybrid; default = slot.score_floor
slot?: string                               // default = _meta.embedding_default_slot
```

Validation in `api/src/search/index.ts`: `mode` in set; `min_score` in [0, 1]; `slot` must be a
key of `_meta.embedding_slots` *and* loaded by the service per `/info`; semantic/hybrid reject the
UUID fast path (an id is not text); `boosts` are ignored in semantic mode and apply to the lexical
sub-query in hybrid mode; `offset` must be 0 in hybrid mode on 2.17 (see Open Questions) — the
route returns 400 with a clear message rather than silently paginating wrong.

Query composition (`opensearch.ts`), reusing the existing filter builder unchanged:

```jsonc
// mode=semantic — k-NN with per-request ef_search, filters INSIDE the knn clause (R1);
// the slot's floor is applied to the returned scores by the API. The radial (min_score) form
// was measured at corpus scale and rejected: on the Lucene engine it accepts no ef_search and
// lands at 0.82 recall, while k + ef_search 256 reaches 0.999 (docs/benchmarks/semantic-search-poc.md).
{ "size": limit, "from": offset,
  "query": { "knn": { "emb_<slot>": {
      "vector": <query embedding>,
      "k": max(limit + offset, K_MIN),               // k caps hits regardless of size
      "method_parameters": { "ef_search": EF_SEARCH }, // MUST be sent: the index default is ignored by the Lucene engine
      "filter": { "bool": { "filter": [ /* scope, space, types, tags, canonical, deleted */ ] } } } } },
  "_source": { "excludes": ["emb_*"] } }
// then: hits.filter(h => h._score >= min_score)

// mode=hybrid — one request, server-side fusion
{ "size": limit,
  "query": { "hybrid": { "queries": [
      <existing function_score lexical query, unchanged>,
      { "knn": { "emb_<slot>": { "vector": ..., "k": HYBRID_K, "filter": { ...same filters... } } } } ] } } }
// sent with ?search_pipeline={prefix}hybrid_minmax
```

New module `api/src/services/embedding/client.ts`: `embedQuery(slot, text)` POSTs
`{slot, purpose: "query", texts: [text]}` to `EMBEDDING_SERVICE_URL` (an in-cluster address),
2 s timeout, Effect-typed errors, traced. Slot registry: `loadSlots()` reads `_meta` at boot (and
every 5 min); `verifySlot()` calls `/info` and compares descriptors (R4). If the service is unset
or verification fails, `mode=semantic|hybrid` return 503 `SEMANTIC_SEARCH_UNAVAILABLE`; lexical
is never affected (R7).

Response additions on every result and on the envelope:

```ts
// envelope
mode: "lexical" | "semantic" | "hybrid"
embeddingSlot?: string                 // present for semantic/hybrid
// per result
semanticScore?: number                 // semantic: the knn _score on the slot's scale
relevanceScore: number                 // unchanged meaning for lexical; fused score for hybrid
```

Scores are never renamed across modes: `semanticScore` is only ever a similarity on the named
slot's scale; `relevanceScore` in hybrid mode is a min-max-normalized fusion and is documented as
not comparable across queries. `QUERY_ARCHITECTURE.md` gets a "Semantic and hybrid modes"
section.

Guardrails: query text cap stays 250 chars; `K_MIN = 50` and `EF_SEARCH = 256` (measured: 0.999
recall at 8 ms median on 192k filtered docs; 512 buys the last 0.001 for 2 ms); `HYBRID_K = 100`
with the same `ef_search`; `MAX_LIMIT = 100` unchanged; every k-NN clause carries
`method_parameters.ef_search` because the Lucene engine otherwise searches with ef = k and recall
drops to ~0.86; the embedding call is inside the existing request budget and shows in the
canonical request log as its own span.

## search-admin additions

| Command | Does |
|---|---|
| `add-embedding-slot --bundle <dir>` | loads `bundle.json`, verifies artifact hashes, computes the slot id, `put_mapping` for the three fields, writes `_meta.embedding_slots.<slot>` |
| `set-default-slot <slot>` | writes `_meta.embedding_default_slot` (the API picks it up on refresh) |
| `list-slots` | prints slots, default, whether the service has each loaded (`/info`), and per-slot coverage (`exists: emb_<slot>` count vs in-scope count) |
| `ensure-search-pipeline` | idempotent `PUT` of the hybrid pipeline for the environment prefix |
| `full-migration` | unchanged; the new mapping simply includes `index.knn` and the slot fields |

Per-environment k8s jobs follow the existing pattern in `search-indexer-deploy/k8s/*/jobs/`.

## Model rotation procedure (D9)

Rehearsed on staging in P3 before any real rotation.

1. Build bundle B; build image `embedding-service:<slotA>,<slotB>`; roll the service (both slots
   loaded; `/info` lists both).
2. `search-admin add-embedding-slot --bundle B` (additive; A keeps serving).
3. Deploy a second `embedding-indexer` with `EMBEDDING_SLOT=B`; it backfills B in parallel
   (optionally with `EMBED_BACKFILL_BACKEND=extraction_api` if extraction-api has bundle B).
4. Run the evaluation harness against A and B (`slot=` parameter); compare.
5. `set-default-slot B`. Callers that pinned `slot=A` keep working.
6. After a grace period: stop A's indexer; roll the service without A; remove A's fields in the
   next scheduled index migration.

No step reindexes, no step overwrites a vector, and at every step every response says which
slot answered.

## Data-flow walkthroughs

- **New named entity.** search-indexer upserts the doc with `indexed_at=t1`. Within one poll
  interval the embedding-indexer sees it, computes the text, embeds via the service, CAS-writes.
  Searchable lexically at t1, semantically at t1 + poll + embed.
- **Name changes while a vector write is in flight.** Poller read name₁ at t1; search-indexer
  writes name₂ at t2 with `indexed_at=t2`; poller's CAS at t3 compares name₂ ≠ name₁ → noop;
  next poll (range ≥ checkpoint − overlap) reads name₂ → embeds → writes. No stale vector is ever
  visible.
- **Name unset.** Doc loses `name`, `indexed_at` stamped; poller classifies it out-of-scope →
  removes the slot fields. The doc has no vector and cannot match semantically.
- **Delete / restore.** Tombstone sets `deleted=true` (stamped); CAS script no-ops; the filter
  already hides the doc. Restore clears `deleted`; text unchanged → hash equal → nothing to do.
- **Service outage.** Follow loop backs off; `emb_<slot>_at` stops advancing; stored vectors
  keep serving lexical-plus-stored-vector queries, but *new* semantic queries return 503 while
  lexical keeps working. Recovery needs no operator action. Nothing outside the cluster is
  involved.
- **Backfill via extraction-api.** The indexer enqueues pages as `embeddings.embed` runs (256
  texts each, up to N in flight), polls, verifies each result's descriptor, CAS-writes. A run that
  echoes a different artifact hash is rejected and counted; sustained failure flips the backend to
  `service`. The follow loop is untouched throughout.
- **Reindex (`full-migration`).** Vectors travel in `_source`; the new index has them on day one.

## Invariants

1. `emb_<slot>` on a document is `embed_slot(text_template(name, description))` for the text the
   document had when `emb_<slot>_src_hash` was written, and that hash equals the current text's
   hash whenever the poller is caught up.
2. A document outside the scope policy has no slot fields.
3. The API never returns a `semanticScore` without an `embeddingSlot`.
4. The writer and the query side of a slot have verified the same artifact hashes from the
   running service in the current process lifetime.
5. Filters are applied inside the k-NN clause; there is no code path that post-filters an
   approximate neighbor list.
6. No vector is written whose producing descriptor (in-cluster or external) differs from the
   slot's descriptor.

## Sizing (to be replaced by measurements in P0/P1)

- Measured on the 321k-document claims corpus (single segment): raw vectors 471 MB (`.vec`),
  HNSW graph 17 MB (`.vex`), so the vector working set is ≈ **0.5 GB per slot** in page cache;
  whole index 2.4 GB, of which vectors inside `_source` are ≈ 0.6–0.7 GB. Rule of thumb that
  matched: `N × dims × 4 B` for the working set, graph overhead ≈ 4 %.
  - claims only: ≈ 0.5 GB RAM, ≈ 2.4 GB disk with today's mapping (≈ 1.7 GB without vectors in `_source`).
  - every named document (unknown count; the September sizing found 49.6 M entity ids but far
    fewer connected, named ones): measure with `list-slots` coverage before widening scope.
- Service throughput (measured, see Spike results): 660 texts/s on 10 laptop cores, 400 texts/s
  at 2 intra-op threads on bare metal, **278 texts/s inside a 2-CPU Linux container** for bge-small
  at batch 64. Cloud x86 vCPUs differ from M4 cores, so plan with 200 texts/s per 2-CPU pod until
  measured on the cluster; scale the HPA for backfill windows.
- Backfill time = in-scope docs ÷ aggregate throughput. At the 200 texts/s planning number,
  320 k claims ≈ 27 min and 5 M documents ≈ 7 h on one pod; at the measured 400/s, half that. The
  extraction-api backend adds off-cluster capacity for the large case without touching the
  service's query lane.
- Query cost: one in-cluster `/embed` call (≈ 5–20 ms for a small model) + one k-NN query (single
  shard, filtered HNSW; geo-lens saw 11–22 ms on 316 k vectors in Neo4j — OpenSearch expected
  similar).
- Steady-state embed load is tiny: gaia's own numbers put daily entity bumps in the tens of
  thousands, i.e. < 1 text/s.

## Configuration reference

embedding-service: `EMBEDDING_MODELS_DIR=/models`, `EMBEDDING_SLOTS` (csv of slot ids to load),
`EMBEDDING_INTRA_OP_THREADS`, `EMBEDDING_MAX_BATCH=256`, `EMBEDDING_MAX_TEXT_CHARS=8000`,
`EMBEDDING_MAX_INFLIGHT=4`, `EMBEDDING_QUERY_INFLIGHT=2`, `PORT=8080`, `SENTRY_*`.

embedding-indexer: `OPENSEARCH_URL`, `INDEX_ALIAS` (`entities`), `ENVIRONMENT` (prefix),
`EMBEDDING_SERVICE_URL`, `EMBEDDING_SLOT`, `EMBED_FOLLOW_BACKEND=service` (only value),
`EMBED_BACKFILL_BACKEND=service|extraction_api`, `EMBED_POLL_INTERVAL_MS=5000`,
`EMBED_OVERLAP_S=30`, `EMBED_PAGE_SIZE=500`, `EMBED_BATCH_SIZE=64`, `EMBED_CONCURRENCY=2`,
`EMBED_MAX_DOCS_PER_CYCLE=20000`, `EMBED_SCOPE_TYPE_IDS`, `EMBED_SCOPE_SPACE_IDS`,
`EMBED_REQUIRE_NAME=true`, `EMBED_SKIP_DELETED=true`, `EMBED_LRU_SIZE=100000`,
`EMBED_RATE_LIMIT_TPS`, `EXTRACTION_API_URL`, `EXTRACTION_API_KEY`,
`EXTRACTION_API_BATCH_SIZE=256`, `EXTRACTION_API_POLL_MS=500`, `EXTRACTION_API_MAX_INFLIGHT=8`,
`EXTRACTION_API_MAX_FAILURES=5`, `HEALTH_PORT=8080`, `SENTRY_*`.

api: `EMBEDDING_SERVICE_URL` (unset ⇒ lexical only), `EMBEDDING_QUERY_TIMEOUT_MS=2000`,
`EMBEDDING_SLOTS_REFRESH_S=300`.

---

# Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Vectors live in the existing OpenSearch `entities` index, one `knn_vector` field per slot | the view already holds the text and every filter; no new datastore; hybrid fusion is one query |
| D2 | Embeddings are computed by a **gaia-owned Rust service** (`embedding-service`, ONNX runtime, models baked in) running in the cluster; both the indexer and the API call it; a shared `embedding` crate carries descriptor, template and provider code | no external dependency on the read path or in the indexer (Goal 5); one language and release train; the descriptor logic is *linked*, not re-implemented; the same ONNX artifacts geo-lens used give vector-level parity |
| D3 | Immutable **descriptor** (artifacts pinned by content hash) hashed to a **slot id**; stored in index `_meta`; verified against the service's `/info` at startup; named in every response | "never implicit" as an enforced invariant, not a convention; a model *name* is not an identity |
| D4 | Trigger = incremental poll over `indexed_at`; search-indexer starts stamping it | the field is mapped but never written today; polling the view has precedent (tally worker); removes GRC-20 re-derivation and ordering races |
| D5 | Text source = the search document, never the edit | one definition of "the entity's name"; the CAS guard compares text, so search-indexer needs no embedding knowledge |
| D6 | v1 text template = `name + "\n\n" + description`; nameless docs get no vector; scope by type allowlist | claims are name-only; description helps everything else; scope is the real memory lever |
| D7 | Lucene HNSW engine, `cosinesimil`, filters **inside** the `knn` clause, `ef_search` sent on every request, floor applied client-side | exactness contract (efficient filtering with exact fallback); no native memory to size; measured at corpus scale: ≥ 0.99 recall only with per-request `ef_search`, radial `min_score` has no recall control on this engine |
| D8 | Scores are on the slot's `(1 + cos) / 2` scale; floors live in the descriptor | a threshold is a property of a model; geo-lens's 0.85 transfers only to the same artifacts |
| D9 | Rotation = new slot, parallel backfill, harness comparison, default flip, later retirement | zero downtime, zero reindex, rehearsed on staging before first real use |
| D10 | Reranking / stance / query understanding deferred; `mode` and per-stage score fields are the extension points | agreed scope for this draft |
| D11 | `EmbedBackend` in the indexer: `service` is the only follow-mode backend; `extraction_api` is an **opt-in backfill** backend whose every result is descriptor-verified and which falls back to `service` on sustained failure | reuses extraction-api's queue where a queue helps (large one-time loads, off-cluster capacity, rate limiting, visibility) without making it a dependency of steady-state indexing |
| D12 | v1 slots are local ONNX models only; hosted providers stay behind the `HttpProvider` feature flag | a hosted slot would put an external call on the query path; that is a separate decision |

# Alternatives considered

- **Third-party inference server (Hugging Face Text Embeddings Inference) in the cluster** — rev 1's
  choice. Fast and model-agnostic, but a second toolchain and release train, and the descriptor
  logic would live beside it rather than be linked by the services that depend on it. Kept as a
  possible `HttpProvider` target later; not the v1 runtime.
- **extraction-api as the sole embedding compute plane** (sync `/embed` for queries, task for
  bulk). One runtime, zero drift, but an out-of-cluster, other-org dependency on a public read
  path and inside the indexer's steady-state loop, with cross-cloud latency on every semantic
  query. Rejected for those paths; retained as the backfill backend (D11).
- **extraction-api as the indexer itself** (a Hatchet cron scanning OpenSearch and writing
  vectors). Least new code, but it names gaia's index and writes to gaia's store, which breaks the
  app-agnostic and stateless rules that layer follows, and puts search freshness on a Railway cron.
- **Kafka-triggered embedding indexer with shared GRC-20 derivation.** Sub-second freshness, but
  needs a second copy of the search consumer's text derivation and a catch-up race against
  search-indexer, so it needs the poll/reconcile loop anyway. Kept as the upgrade path.
- **Embedding inside search-indexer's processor.** Puts a model in the hot path; an outage stalls
  all search indexing. Rejected.
- **Embedding in-process in the indexer (link the crate, no service).** Removes an in-cluster
  hop for documents, but the API still needs a query-side runtime, so a service exists anyway;
  two runtimes of the same crate is acceptable, one is simpler. Revisit if the hop ever matters.
- **OpenSearch-native (ml-commons + neural-search ingest pipeline).** Ingest pipelines do not run
  on the `update`/bulk-update path search-indexer uses, and it ties the model lifecycle to the
  cluster (unknown flavor in production).
- **`candle` instead of the ONNX runtime.** Pure Rust, but slower on CPU and more per-model code;
  and it would not run the identical artifacts that give parity with geo-lens and with the
  extraction-api backend. Confined behind the provider trait.
- **pgvector.** Only earns its place for similarity joined against arbitrary property values,
  which is out of scope; adds load to the Postgres path with the open p99 issue.
- **Keep geo-lens, feed it from Kafka.** Removes its sync engine but keeps a third store; ruled
  out by the decision to bring this inside gaia.

# External requirements (optional, for the backfill backend only)

extraction-api (`geo-explorers/extraction-api`), if `EMBED_BACKFILL_BACKEND=extraction_api` is
ever used:

1. Port geo-lens's `src/embeddings` module (provider protocol with `purpose`, the fastembed
   adapter, the provider registry) into `src/infrastructure/embeddings/`, replacing the dormant
   Ollama-only `EmbeddingService`. Load models from the **same bundles** (same files, same
   hashes) so the artifact hashes match gaia's descriptor.
2. Add an app-agnostic task `embeddings.embed`: input `{ descriptor, purpose, texts[] }`, output
   `{ descriptor (as honored, with artifact hashes), vectors[][] }`. The handler refuses a
   descriptor whose artifacts it does not have. No store, no caller names, no gaia knowledge —
   it satisfies the layer's stateless and app-agnostic rules.
3. Its own concurrency cap and rate-limit key (`embed_local`); embedding is CPU-bound while the
   existing tasks are I/O-bound, so a separate worker service from the same image is likely.
4. An API key for caller `gaia` in `API_KEYS`; gaia holds it as a k8s secret on the
   embedding-indexer only.

None of this is on the critical path for P0–P2.

# Milestones

| Phase | Deliverable | Exit criterion |
|---|---|---|
| P0 spike (local compose) | `embedding` crate + `embedding-service` serving one bundle; `entities_v{N+1}` with `index.knn` + slot; embedding-indexer backfills local data via `service`; semantic query via curl | vectors for a fixed text set match geo-lens (Python fastembed, same artifacts) to ≤ 1e-4 cosine; filtered k-NN top-k equals brute-force `script_score` top-k on a space filter (R1); service throughput and p95 query latency measured |
| P1 staging | `full-migration` to `v{N+1}`; service + indexer deployed, scope = claims type; `search-admin` slot commands; API `mode=semantic` | geo-lens parity: the four P1 queries (verbatim / paraphrase / hedged / novel) give the same equivalents from `/search?mode=semantic` as from geo-lens `/query`; coverage = 100 % of in-scope docs |
| P2 | `mode=hybrid` + pipeline; production rollout of P1+P2 | hybrid returns for the staging query set; production coverage complete; dashboards for cycle stats, service and query latency |
| P3 rotation drill (staging) | bundle B with a different model (e.g. a Matryoshka-capable 2025 model at 256–768 d); service with two slots; parallel backfill; harness comparison; default flip; A retired | procedure documented in the runbook and executed once end to end |
| P4 | widen scope beyond claims by type allowlist, driven by `list-slots` coverage and memory; if the backfill is large, bring up the extraction-api backend (External requirements) and run it descriptor-verified | per-slot RAM within cluster budget; a backfill completed through `extraction_api` with zero descriptor mismatches |

The runtime half of P0 was completed on 2026-09-29 (see Spike results under the `embedding`
crate): parity, throughput and static linking are measured facts, not assumptions. The k-NN half
was completed the same day on the local 2.17.1 stack (see Spike results under Index changes),
and the corpus-scale step 0 followed (`docs/benchmarks/semantic-search-poc.md`): 321k real
documents, recall/latency per shape, storage breakdown, the k-mode decision for semantic mode,
and store-level parity with geo-lens. What remains for P1 is the service-level parity check.

The evaluation harness (a golden set of queries with expected and unexpected results, recall@k
and floor calibration per slot) is built in P1 and is a prerequisite for P3. It is also the
foundation the deferred "beyond similarity" track needs.

# Open questions

1. **Production OpenSearch flavor.** Managed or self-hosted, and are the k-NN plugin and search
   pipelines enabled? Everything above assumes the official 2.17 distribution as in compose.
2. **ONNX runtime distribution in the image — resolved 2026-09-29.** The default `ort` feature
   downloads a SHA-256-verified prebuilt at build time and links it **statically**; the binary
   carries the runtime. Build stages need egress to `cdn.pyke.io`; `ORT_LIB_LOCATION` is the
   vendored alternative if that egress is ever unwanted. The runtime version is recorded in
   `/info` and is not part of the slot id (see Spike results). **Consequence:** the
   embedding-service image is built on Debian trixie (GCC 14), because the prebuilt runtime needs
   libstdc++ ≥ GCC 13 on both architectures.
3. **Hybrid pagination on 2.17 — resolved 2026-09-29.** Verified unsupported on 2.17.1 (the engine
   rejects `from > 0` with "pagination is not supported with hybrid query"); v1 returns 400 for
   `offset > 0` in hybrid mode. Semantic mode pages normally (radial `knn` accepts `from`).
4. **Vectors in `_source`.** Kept for v1 so `reindex` carries them; measured cost ≈ 0.6–0.7 GB per
   321k documents (25–30 % of the index). If disk becomes the constraint, exclude them and make
   `full-migration` trigger a re-embed instead.
5. **Per-space duplicates.** One document per `(entity, space)` means the same text may be
   embedded once per space; the LRU and in-page dedupe remove the compute, not the storage. Is a
   per-entity vector store (one vector, joined at query time) worth it later?
6. **First bundle.** bge-small with geo-lens's exact artifacts gets vector-level parity for free;
   a stronger small model is the P3 candidate. Decide with the harness, not here.
7. **Scope policy source.** Env allowlist in v1. Should scope eventually be declared in the graph
   (a property on type entities), so the indexer needs no redeploy to widen it?
8. **Widening the indexed relation set** (needed for stance/topic filters in the deferred track)
   changes document size and update fan-out; out of scope but worth noting before P4 widens scope.

# Files touched (expected)

| Area | Path |
|---|---|
| new lib crate | `embedding/` (descriptor, slot id, text template, `EmbeddingProvider`, `OnnxLocalProvider`, bundle loading) |
| new service | `embedding-service/` (+ `Dockerfile` on `rust:1.92-trixie` / `debian:trixie-slim` baking `/models/<slot>/`, `k8s/` Deployment / Service / HPA), `scripts/make-bundle.sh` |
| new indexer | `embedding-indexer/` (+ `Dockerfile`, `k8s/`), `EmbedBackend` with `service` and `extraction_api`; bookworm is fine here since it links no ONNX runtime |
| shared | `search-indexer-shared` unchanged; the search crates depend on `embedding` only through `search-admin` |
| mapping | `search-indexer-repository/src/opensearch/index_config.rs` |
| stamp | `search-indexer-repository/src/opensearch/provider.rs` (`build_update_doc`), `bulk.rs`, `unset_document_properties.rs` |
| admin | `search-admin/src/commands/{add_embedding_slot,set_default_slot,list_slots,ensure_search_pipeline}.rs` |
| api | `api/src/services/search/types.ts`, `opensearch.ts`, `QUERY_ARCHITECTURE.md`, `api/src/search/index.ts`, `api/src/services/embedding/client.ts`, `api/main.ts` |
| infra | `Cargo.toml` (three members), `docker-compose.yml` (two services), `.github/workflows/embedding-{service,indexer}-*.yml`, `docs/runbooks/deployment.md` (rotation procedure) |
| docs | this file; `README.md` subsystem table (+ `embedding`, `embedding-service`, `embedding-indexer`) |
| external (optional) | extraction-api: `src/infrastructure/embeddings/`, `src/tasks/embeddings_embed.py`, registry entry, `API_KEYS` entry |
