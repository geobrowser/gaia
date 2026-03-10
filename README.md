# Gaia

Knowledge graph data service for the [Geo protocol](https://geobrowser.io/). Gaia ingests onchain events from the Geo blockchain, transforms and indexes them, and serves the resulting knowledge graph through a GraphQL/REST API.

## Architecture

```
                              +-----------------------------------------------------------+
                              |  Hermes (event streaming)                                 |
                              |                                                           |
+--------------+              |  +----------------+    +--------------+                   |
|  Blockchain  |--------------|--->  hermes-      |--->| hermes-relay |                   |
|    (Geo)     |              |  |  substream     |    |   (library)  |                   |
+--------------+              |  +----------------+    +------+-------+                   |
                              |                               |                           |
                              |                  +------------+                           |
                              |                  |            |                           |
                              |                  v            v                           |
                              |  +-------------------+  +----------+                      |
                              |  | hermes-pipeline   |  |  atlas   |                      |
                              |  | (all events)      |  | (graphs) |                      |
                              |  +--------+----------+  +----+-----+                      |
                              |           |                   |                           |
                              +-----------+-------------------+---------------------------+
                                          |                   |
                                          v                   v
                                       Kafka              Kafka
                                                              |
                     +----------------------------------------+
                     |
                     v
+--------------------------------------------------------------------+
|  Indexers (Kafka --> PostgreSQL / OpenSearch)                       |
|                                                                    |
|  kg-indexer . search-indexer . actions-indexer . scoring-service    |
+------------------------------+-------------------------------------+
                               |
                               v
                     +-------------------+
                     |   Gaia API        |
                     |   (Bun + Hono)    |
                     |                   |
                     |  /graphql         |<-- PostGraphile
                     |  /versioned/*     |<-- Temporal queries
                     |  /proposals/*     |<-- Governance
                     |  /profile/*       |<-- User profiles
                     |  /search/*        |<-- OpenSearch
                     |  /ipfs/*          |<-- IPFS uploads
                     |  /health/*        |<-- K8s probes
                     +-------------------+
```

### Subsystems

| Domain              | Crates                                                                                                                                                                            | Description                                            |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| **Hermes Pipeline** | [hermes-pipeline](hermes-pipeline/), [hermes-relay](hermes-relay/), [hermes-kafka](hermes-kafka/), [hermes-substream](hermes-substream/), [hermes-ipfs-cache](hermes-ipfs-cache/) | Streams blockchain events to Kafka topics              |
| **Graph**           | [atlas](atlas/)                                                                                                                                                                   | Computes canonical space topology from trust events    |
| **Indexers**        | [kg-indexer](kg-indexer/), [search-indexer](search-indexer/), [actions-indexer](actions-indexer/), [scoring-service](scoring-service/)                                            | Consume Kafka topics, write to PostgreSQL / OpenSearch |
| **API**             | [api](api/)                                                                                                                                                                       | GraphQL + REST read layer over indexed data            |
| **Governance**      | [proposal-executor](proposal-executor/)                                                                                                                                           | Onchain proposal execution                             |
| **Infrastructure**  | [hermes](hermes/) (docker-compose + k8s), [monitoring](monitoring/), [search-indexer-deploy](search-indexer-deploy/)                                                              | Local dev environment, observability, deployment       |

## Local Development

### Prerequisites

- [Rust](https://www.rust-lang.org/) (see `rust-toolchain.toml`)
- [Bun](https://bun.sh/)
- [PostgreSQL](https://www.postgresql.org/)
- [Docker](https://www.docker.com/) (for Kafka)

### 1. Start Infrastructure

```bash
# Kafka + Kafka UI
docker compose -f hermes/docker-compose.yaml up -d

# PostgreSQL (for the API and indexers)
docker compose -f api/docker-compose.yml up -d
```

Kafka UI is available at `http://localhost:8080`.

### 2. Run Rust Services

Each Rust service runs independently with `cargo run -p <crate>`:

```bash
# Hermes pipeline (all blockchain events → Kafka)
KAFKA_BROKER=localhost:9092 cargo run -p hermes-pipeline

# Atlas (canonical graph computation → Kafka)
KAFKA_BROKER=localhost:9092 cargo run -p atlas

# Knowledge graph indexer (Kafka → PostgreSQL)
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia \
  KAFKA_BROKER=localhost:9092 cargo run -p kg-indexer

# Search indexer (Kafka → OpenSearch)
OPENSEARCH_URL=http://localhost:9200 \
  KAFKA_BROKER=localhost:9092 cargo run -p search-indexer
```

See each crate's README for full configuration and environment variables.

### 3. Run the API

```bash
cd api
cp .env.example .env  # configure env vars (see .env.example for descriptions)
bun install
bun run db:migrate    # run database migrations
bun run start         # start the API server
```

See [api/README.md](api/README.md) for full setup details.

## Documentation

### Conventions

- `docs/` — cross-cutting system documentation
- `<crate>/docs/` — crate-specific docs (decisions, plans, architecture)
- `<crate>/README.md` — crate entry point

When adding a new crate, update the subsystem table above and add a crate README.

### Architecture & Design

- [Hermes Architecture](docs/architecture.md) — event streaming system design
- [API Architecture](docs/api-architecture.md) — API layers, tech stack, query patterns
- [Decision Records & RFCs](docs/decisions/README.md) — central index of all ADRs and RFCs
- [Gotchas](docs/gotchas.md) — known sharp edges and workarounds

### Specifications & RFCs

- [Atlas Canonical Graph Spec](docs/specs/atlas-canonical-graph-spec.md)
- [Canonical Graph Spec](docs/specs/canonical-graph.md)
- [Versioned Diffing Spec](docs/specs/versioned-diffing.md)
- [RFC 0001: Canonical Graph Inputs](docs/rfcs/0001-canonical-graph-inputs.md)
- [RFC 0002: Graph Diff Emission](docs/rfcs/0002-graph-diff-emission.md)
- [RFC 0003: Context-Aware Versioned Diffs](docs/rfcs/0003-context-aware-versioned-diffs.md)

### Operations

- [Staging & Production Runbook](docs/runbooks/staging-production.md)

### Protocol

- [Protocol docs](docs/protocol/)

### Research

- [Research docs](docs/research/)

<details>
<summary>Legacy System (pre-Hermes)</summary>

The following crates are from the pre-Hermes architecture and are sunset. They are not actively maintained and will be removed in a future cleanup:

- `indexer/` — legacy knowledge graph indexer (replaced by kg-indexer + Hermes pipeline)
- `cache/` — legacy IPFS cache (replaced by hermes-ipfs-cache)
- `wire/` — legacy protobuf wire format
- `stream/` — legacy substreams connector
- `indexer_utils/` — legacy indexer utilities

</details>
