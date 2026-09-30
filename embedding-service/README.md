# embedding-service

gaia's in-cluster embedding runtime. It loads hash-verified model **bundles** and serves them
over HTTP to the search API (query embeddings) and the embedding-indexer (document embeddings).
Nothing else in gaia embeds text. Design and measurements:
[`docs/tech-designs/semantic-search.md`](../docs/tech-designs/semantic-search.md).

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/embed` | `{ "slot": "<id>", "purpose": "document" \| "query", "texts": [..] }` → `{ slot, descriptor_hash, dimensions, purpose, vectors, took_ms }` |
| `GET` | `/info` | every loaded slot's descriptor, hash and vector field, plus limits |
| `GET` | `/health/live`, `/health/ready` | liveness; readiness = at least one slot loaded |

Errors are `{ "error": { "code", "message" } }`: `413 batch_too_large` / `text_too_long`,
`404 unknown_slot`, `422 invalid_body` for a malformed body, `500 embedding_failed`.

## Bundles and slots

A bundle is a directory with `bundle.json` (the **descriptor**: model artifacts pinned by SHA-256,
dimensions, pooling, quantization, prompts, text template, score floor) and the artifacts it
lists. The **slot id** is the first 10 hex characters of the SHA-256 of the descriptor's canonical
JSON; the OpenSearch field for a slot is `emb_<slot>`. Change any field and you have a new slot.

Descriptors live in [`bundles/`](bundles/) under a human name; weights are never committed. The
image build runs `embedding-service bundle fetch` for each, which downloads from the pinned source,
verifies every hash, and places the bundle under `/models/<slot>/`. At startup every bundle is
verified again; a mismatch refuses to load.

```bash
embedding-service bundle slot   bundles/bge-small-en-v1.5-q/bundle.json   # print the slot id
embedding-service bundle fetch  --spec bundles/bge-small-en-v1.5-q/bundle.json --out ./models
embedding-service bundle verify ./models/<slot>
```

## Run locally

```bash
cargo run -p embedding-service -- bundle fetch --spec embedding-service/bundles/bge-small-en-v1.5-q/bundle.json --out ./models
EMBEDDING_MODELS_DIR=./models cargo run -p embedding-service -- serve
curl -s localhost:8080/info | jq .slots
curl -s localhost:8080/embed -H 'content-type: application/json' \
  -d '{"slot":"<slot>","purpose":"query","texts":["Bitcoin is a store of value"]}' | jq '.vectors[0][:4]'
```

Tests: `cargo test -p embedding -p embedding-service`. The golden-vector and real-embed tests run
only with `EMBEDDING_TEST_BUNDLE=./models/<slot>`; the rest need no model.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `EMBEDDING_MODELS_DIR` | `/models` | bundle root |
| `EMBEDDING_SLOTS` | all found | comma-separated slot ids to load |
| `EMBEDDING_INTRA_OP_THREADS` | runtime default | ONNX Runtime threads per session; set to the CPU limit |
| `EMBEDDING_BATCH_SIZE` | 64 | texts per inference call |
| `EMBEDDING_MAX_BATCH` | 256 | largest request |
| `EMBEDDING_MAX_TEXT_CHARS` | 8000 | longest text |
| `EMBEDDING_MAX_INFLIGHT` / `EMBEDDING_QUERY_INFLIGHT` | 4 / 2 | concurrent document / query requests |
| `EMBEDDING_QUERY_LANE` | true | second session per slot so queries never wait behind document batches |
| `EMBEDDING_BIND`, `PORT` | `0.0.0.0`, 8080 | |
| `RUST_LOG`, `LOG_FORMAT=json` | `info`, text | logging |

## Image

`rust:1.92-trixie` → `debian:trixie-slim`. Trixie rather than bookworm because the prebuilt ONNX
Runtime needs libstdc++ from GCC ≥ 13 (measured on both architectures). The runtime is linked
statically; the image needs no extra packages.
