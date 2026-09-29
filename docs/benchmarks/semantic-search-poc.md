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
