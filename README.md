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
| **Infrastructure**  | [docker-compose.yml](docker-compose.yml), [hermes](hermes/) (k8s), [monitoring](monitoring/), [search-indexer-deploy](search-indexer-deploy/)                                     | Local dev environment, observability, deployment       |

## Local Development

### Prerequisites

- [Rust](https://www.rust-lang.org/) (see `rust-toolchain.toml`)
- [Bun](https://bun.sh/)
- [Docker Desktop](https://www.docker.com/) or [OrbStack](https://orbstack.dev/) with **≥ 8 GB memory** allocated (OpenSearch needs 2–4 GB alone)
- Docker Compose v2 (for profiles support)

> **First build note:** Rust services compile from source on first `docker compose build`, which can take a while depending on your machine.

### 1. Environment Setup

```bash
cp .env.example .env
# Fill in at minimum: SUBSTREAMS_API_TOKEN, SUBSTREAMS_ENDPOINT, ROOT_SPACE_ID
```

See [.env.example](.env.example) for all variables and descriptions.

### External Dependencies

Gaia runs most infrastructure locally with Docker.
Kafka, PostgreSQL, and OpenSearch come from [docker-compose.yml](docker-compose.yml).

The remaining dependencies are external services:

| Dependency | Current/default provider | Used by | Required env vars | Notes |
| ---------- | ------------------------ | ------- | ----------------- | ----- |
| Substreams | Pinax (`geotest.substreams.pinax.network:443`) | `hermes-pipeline`, `atlas`, `hermes-ipfs-cache`, `actions-indexer` | `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN` | Required for any live blockchain ingestion. |
| IPFS read gateway | Mesh3 by default (`gateway.mesh3.network`) | `hermes-ipfs-cache` | `IPFS_GATEWAY_URL` | Fetches edit and proposal blobs from IPFS. |
| IPFS write gateway | Pinata (`uploads.pinata.cloud`) | `api` `/ipfs/*` routes | `IPFS_KEY`, `IPFS_GATEWAY_WRITE` | Only needed if you want API uploads enabled. |
| Bundler / paymaster | Pimlico (`api.pimlico.io`) | `proposal-executor` | `PIMLICO_API_KEY` | Only needed with `--profile executor`. |
| Chain RPC | Bring your own RPC provider | `proposal-executor` | `RPC_URL` | Only needed with `--profile executor`. |
| Error tracking | Sentry | optional telemetry | `SENTRY_DSN` | Safe to omit locally. |
| Trace export | Axiom | optional Hermes tracing | `AXIOM_TOKEN`, `AXIOM_DATASET` | Only used when Sentry-backed tracing is enabled. |

If you only want the local indexers and API:

- Set `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN`, and `ROOT_SPACE_ID`.
- Set `IPFS_GATEWAY_URL` if you run `hermes-ipfs-cache`.
- Set `IPFS_KEY` and `IPFS_GATEWAY_WRITE` only if you need API uploads.
- Skip `PIMLICO_API_KEY` and `RPC_URL` unless you run `proposal-executor`.

If you run a service natively, pass the same env vars from `.env`.
Docker Compose does not inject them into native `cargo run` or `bun run` commands.

### 2. Start Infrastructure

```bash
# Kafka, PostgreSQL, OpenSearch
docker compose --profile infra up -d
```

### 3. Run Services

**Option A — All services in Docker:**

```bash
# Infrastructure + all application services (first build is slow)
docker compose --profile infra --profile services up
```

**Option B — Infrastructure in Docker, run one service natively:**

```bash
docker compose --profile infra up -d

# Example: run kg-indexer natively
DATABASE_URL=postgresql://postgres:postgres@localhost:5432/gaia \
  KAFKA_BROKER=localhost:9092 cargo run -p kg-indexer
```

**Option C — Run the API natively:**

```bash
docker compose --profile infra up -d
cd api && bun install && bun run db:migrate && bun run start
```

The API is available at `http://localhost:3000`.

### Profiles

| Profile | Command | What it starts |
|---------|---------|----------------|
| `infra` | `docker compose --profile infra up` | Kafka, PostgreSQL, OpenSearch |
| `tools` | `docker compose --profile infra --profile tools up` | + Kafka UI (:8080), OpenSearch Dashboards (:5601) |
| `services` | `docker compose --profile infra --profile services up` | + hermes-pipeline, atlas, kg-indexer, search-indexer, hermes-ipfs-cache, vote-indexer, api, scoring-cronjob |
| `executor` | `docker compose --profile infra --profile services --profile executor up` | + proposal-executor (requires secrets) |

### Common Operations

```bash
# View logs for a service
docker compose logs -f kg-indexer

# Rebuild a single service
docker compose build kg-indexer

# Build all services in parallel
docker compose --profile services build --parallel

# Reset all data (remove volumes)
docker compose --profile infra down -v
```

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
