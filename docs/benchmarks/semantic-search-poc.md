# Semantic search — step 0 corpus-scale proof of concept

Date: 2026-09-29 · Design: [`docs/tech-designs/semantic-search.md`](../tech-designs/semantic-search.md) ·
Scripts: [`semantic-search-poc/`](semantic-search-poc/) (evidence, not product code)

**Question.** Before writing any gaia code: does the design's index shape hold up on a real corpus,
and what do the query shapes cost? Run entirely locally — gaia's compose OpenSearch 2.17.1 (official
image), geo-lens's local Neo4j as the data source, no staging or production access.

## Corpus and load

- Source: geo-lens's `claims` cache (Neo4j, label `C_28644288d488eac0`): **316,605** claim entities,
  **316,557** with a bge-small vector (the 48 nameless ones have none), 2,127 with a description,
  4,851 in more than one space. Vectors are the same artifacts the design pins (slot
  `14a3531800c8` in geo-lens: `qdrant/bge-small-en-v1.5-onnx-q`, 384 d).
- Target: `testnet_entities_poc`, created from **gaia's real mapping** (`search-admin create-index`
  under `ENVIRONMENT=testnet`, then settings and analyzers copied) plus the design's additions:
  `index.knn: true`, `emb_<slot>` (`knn_vector` 384, lucene / hnsw / cosinesimil, m 16,
  ef_construction 128), `emb_<slot>_src_hash`, `emb_<slot>_at`, `_meta.embedding_slots`.
- Expansion: one document per `(entity, space)` as gaia's writer does → **321,408 documents**.
  Types and Tags relations attached as nested `relations` entries (322,921 edges).
- Load (`load.py`): streamed from Neo4j, bulk-indexed with `refresh_interval: -1`,
  **266 s end to end, ~2,000 docs/s, 0 errors**. `_forcemerge` to one segment afterwards: 229 s.

## Storage (single segment after merge)

| Lucene files | MB | What |
|---|---|---|
| `.fdt` | 792 | stored fields (`_source`) — mostly the vectors as JSON text |
| `.dvd` | 525 | doc values (keywords, dates, nested ids, `src_hash`) |
| `.vec` | 471 | raw vectors: 321,408 × 384 × 4 B |
| `.tim` + `.doc` + `.pos` + `.tip` | 572 | inverted index (`search_as_you_type` n-grams dominate) |
| `.vex` | 17 | the HNSW graph |
| **total** | **2,380** | (3,708 MB before the merge, 31 segments) |

Consequences: the vector search working set is `.vec` + `.vex` ≈ **0.5 GB per slot** for this
corpus (Lucene engine, page cache; `_plugins/_knn/stats` reports 0 native graph memory as expected).
Keeping vectors in `_source` costs roughly **0.6–0.7 GB (25–30 % of the index)** — open question 4
in the design now has a number.

## Query shapes (60 query vectors sampled from the 192k-document space; merged index)

Reference for recall is brute-force `script_score` cosine on the same filter. Recall is
**score-based** (a hit counts if its score reaches the reference's k-th score) because the corpus is
full of exact duplicates — identical claims across spaces and identical texts under different ids —
which make id-set recall undercount on ties.

| shape | p50 ms | p95 ms | recall@10 | note |
|---|---|---|---|---|
| knn k=10, no filter, **no `method_parameters`** | 8 | 15 | 0.862 | Lucene engine searches with ef = k unless told otherwise |
| knn k=10, no filter, `ef_search=100` | 4 | 6 | 0.983 | |
| knn k=10, no filter, `ef_search=256` | 5 | 6 | 0.990 | |
| knn k=10, filter space 192k, no ef | 4 | 8 | 0.875 | |
| knn k=10, filter space 192k, `ef_search=256` | 6 | 9 | 0.988 | filter inside the clause (D7) |
| knn k=10, filter space 11k | 12 | 16 | 0.973 | |
| knn k=10, filter Debate tag (475 docs, nested) | 4 | 6 | 1.000 | exact fallback |
| brute force, no filter | 84 | 92 | 1 | reference cost |
| brute force, space 192k | 55 | 66 | 1 | reference cost |
| hybrid: `match(name)` + knn k=100, space 192k, min-max pipeline | 23 | 30 | — | query doc ranks first 58/60 (ties) |

On the unmerged 31-segment index the same shapes were ~1.5–2× slower at the same recall.

**Finding 1 — `ef_search` must be sent per request.** The index default
(`index.knn.algo_param.ef_search = 100`) is not applied by the Lucene engine on 2.17.1; without
`method_parameters: {ef_search: N}` recall is 0.86–0.88. With 256 it is ≥ 0.99 for one to two
extra milliseconds. The API sets it on every k-NN clause.

## Semantic mode: radial vs k + client-side floor (floor 0.85, size 50, space 192k)

| shape | p50 ms | p95 ms | recall vs brute force above floor |
|---|---|---|---|
| radial `min_score`, no ef | 6 | 25 | 0.825 |
| radial `min_score` + `ef_search` | — | — | **rejected by the engine**: "Parameters not valid for [LUCENE]:[hnsw]:[min_score] combination" |
| k=50, no ef, floor applied client-side | 20 | 26 | 0.967 |
| **k=50, `ef_search=256`, floor client-side** | **8** | **14** | **0.999** |
| k=100, `ef_search=256`, floor client-side | 8 | 12 | 0.999 |
| k=50, `ef_search=512`, floor client-side | 10 | 15 | 1.000 |
| k=10, `ef_search=256`, floor client-side (size 10) | 6 | 8 | 1.000 |

**Finding 2 — radial search is out.** On the Lucene engine it has no recall control and lands at
0.82. Semantic mode uses `k = max(from + size, K_MIN)` with `ef_search`, the filter inside the
clause, and the slot's floor applied to the returned scores. Paging works through `from`/`size`
with `k ≥ from + size` (k caps hits regardless of `size`).

## Parity with geo-lens at the store level

20 query vectors, top-10 from the PoC index (collapsed by entity) vs geo-lens's Neo4j vector index
with exact rescoring, the way its `vector` strategy answers: **top-1 identical by id and score
20/20**, max |Δscore| over the top-10 0.0021 (mean 0.0001), id-set overlap 0.99 (ties again). The
imported vectors are faithful and the two engines agree; the service-level parity check (the four
P1 queries through `/search?mode=semantic` vs geo-lens `/query`) remains the P1 exit criterion.

## Reproduce

1. geo-lens: `docker compose up -d neo4j` (the claims cache lives in `neo4j-data/`).
2. gaia: `docker compose --profile infra up -d opensearch` — on an Apple M4 with Docker Desktop 24
   add a compose override with `_JAVA_OPTIONS=-XX:UseSVE=0` for the `opensearch` service (the
   image's JDK 21.0.4 aborts with SIGILL otherwise).
3. `ENVIRONMENT=testnet cargo run -p search-admin -- create-index --version 0`, then derive the PoC
   index as in the "Corpus and load" section (the settings/mappings merge is a dozen lines of
   Python; see the design doc for the exact additions).
4. `load.py`, `bench.py [n] [--forcemerge]`, `bench_radial.py`, `parity_store.py` from the geo-lens
   venv (needs `neo4j` and `httpx`, both geo-lens dependencies).

---

## Step 4–6 follow-up: the real path over the same corpus (2026-09-30)

Same local stack, now through the components that will ship rather than a loader script:
`embedding-service` (release build) → `embedding-indexer` (release build, `EMBED_ONCE`) → the
API's `/search?mode=semantic|hybrid`. The slot is the committed bundle `79502860cd`, registered on
the PoC index with `search-admin add-embedding-slot --index testnet_entities_poc`.

### Backfill through the indexer

| | |
|---|---|
| Documents | 321,408 (every named document; scope = all) |
| Wall time | ≈ 35 min in `--once` mode, page 500, batch 128 |
| Rate | ≈ 150 docs/s with OpenSearch, the service and the indexer sharing one laptop; the service alone did 660 texts/s on this machine, so the indexer's serial page loop (search → embed → bulk write) is the ceiling here, and pipelining pages is the obvious next step |
| Interruptions | one: the Docker VM hit OpenSearch's flood-stage watermark mid-run (index read-only, 429s). The indexer classified it transient and, in `--once` mode, exited; it now retries transient errors with backoff instead. After freeing disk it resumed from its persisted cursor with no duplicate work |
| Handover | `backfill_complete` → `follow` with the checkpoint at the backfill start; the first follow pass scanned 0 |
| CAS no-ops | 0 (no text changed under the writer during the run) |

### Vector parity with geo-lens, real path

geo-lens's vectors were imported into the same documents (`emb_14a3531800c8`, text = name only).

| Documents | n | min cos | p50 | max |
|---|---|---|---|---|
| name only (identical text) | 300 | **0.999999** | 1.000000 | 1.000000 |
| name + description (template now includes the description) | 300 | 0.68 | 0.91 | 0.995 |

The first row is the claim the design rests on: the shipped path — text template, service,
indexer, index — reproduces geo-lens's vectors. The second row is the intended effect of the
`name_description_v1` template, not drift.

### Service-level parity (the P1 exit criterion)

Ten natural-language queries through `GET /search?mode=semantic&limit=10&min_score=0` against the
API, and the same texts through geo-lens `POST /caches/claims/query` (vector strategy) on its Neo4j
mirror, both running locally:

| | |
|---|---|
| Top-1 identical (id, or score within 0.002) | **10 / 10** |
| Mean top score difference | 0.0000 |
| Mean id overlap @10 | 0.95 (the rest are ties among duplicate claims and description-template documents) |

### API behavior observed

Semantic and hybrid answer with `mode` and `embeddingSlot`; `min_score` above the floor returns
nothing when nothing qualifies; scope (`SPACE_SINGLE`) and `tag_ids` filters apply inside the k-NN
clause; lexical mode is byte-for-byte the previous contract; hybrid with `offset` → 400, unknown
slot → 400, unknown mode → 400, an id as query text → 400, no embedding service → 503
`SEMANTIC_SEARCH_UNAVAILABLE`.

### Latency after the backfill

The backfill rewrote every document, leaving dozens of unmerged segments; Lucene walks one HNSW
graph per segment, so semantic queries measured ≈ 440 ms server-side right after it (geo-lens's
single Neo4j index: ≈ 50 ms). After `_forcemerge` to one segment (5.3 GB with both vector fields), the same ten queries:

| | gaia `/search?mode=semantic` | geo-lens `/query` |
|---|---|---|
| server-side p50 | **22 ms** | 18 ms |
| end-to-end HTTP, semantic (embed call + k-NN + type/space/image lookups) | 21–24 ms | — |
| end-to-end HTTP, hybrid (lexical + k-NN, fused by the pipeline) | 29–62 ms | — |

Operationally: a large backfill should be followed by a merge, or scheduled where the merge policy
catches up, before latency is judged. Steady-state follow writes are small and merge normally.

## Step 8: the evaluation harness on the same index (2026-09-30)

`search-admin eval-slot --index testnet_entities_poc --embedding-service http://localhost:8090`
(golden set `search-admin/golden/testnet-debate-claims.json`: 36 queries — 1 verbatim,
18 paraphrase, 10 contrastive, 2 hedged, 1 negation, 4 novel — over 37 Debate-tagged claim names,
all present). Query shape = the api's: k 50, ef_search 256, non-deleted filter inside the clause.
Full report: `eval-79502860cd.json` beside this file.

| | |
|---|---|
| recall@10 (32 scored queries) | 0.969 |
| MRR | 0.822 |
| contrastive pairs | 17 / 19 |
| expected hits | n 33, min 0.870, median 0.926 |
| reject hits | n 18, min 0.828, median 0.889, max 0.974 |
| novel top-1 | n 4, 0.802 – 0.834 |
| floor 0.85 | 0 expected dropped, 0 novel admitted; suggested 0.852 |

Failures: the negation query ("should *not* stop funding Ukraine") puts the negated claim first
at 0.974 (target 6th); the Hormuz paraphrase's target is 69th by brute force behind 50 Hormuz
claims (not an HNSW miss). Both are the deferred D10 track, now with numbers. Ten of the
paraphrases have the target at rank 1; the hedged AI-jobs query has it 6th behind five closely
related claims. Wall time for the run: 36 embeddings in one request plus 36 k-NN queries and 37
presence counts, a few seconds.

To reproduce, the index and service from "Reproduce" above, then the command; `--json` writes the
report. The gate (`--min-recall 0.9`) passed; `--gate-contrastive` would have failed it, which is
the intended reading of that flag until reranking exists.

