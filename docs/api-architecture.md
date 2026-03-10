# API Architecture

This document describes the architecture of the Gaia API, the read layer that serves the knowledge graph, governance, versioning, search, and profile data to consumers.

## Overview

The API is a TypeScript service running on Bun. It serves two query paradigms from the same process:

1. **PostGraphile GraphQL** (`/graphql`) — auto-generated from the PostgreSQL schema, used for general-purpose knowledge graph queries
2. **Custom REST endpoints** — purpose-built for use cases where PostGraphile is insufficient (versioning, governance status, profiles, search, IPFS uploads)

The API is read-only. All data enters through the Rust indexer pipeline (Kafka → indexer → PostgreSQL). The API simply reads from PostgreSQL and OpenSearch.

```
Kafka Topics
    │
    ├── kg-indexer (Rust) ──────────────► PostgreSQL
    ├── search-indexer (Rust) ──────────► OpenSearch
    ├── vote-indexer (Rust) ────────────► PostgreSQL
    └── actions-indexer (Rust) ─────────► PostgreSQL
                                              │
                                              ▼
                                     ┌─────────────────┐
                                     │   Gaia API       │
                                     │   (Bun + Hono)   │
                                     │                  │
                                     │  /graphql        │◄── PostGraphile (auto-generated)
                                     │  /versioned/*    │◄── Temporal entity queries
                                     │  /proposals/*    │◄── Governance status
                                     │  /profile/*      │◄── User profiles
                                     │  /search/*       │◄── OpenSearch proxy
                                     │  /ipfs/*         │◄── IPFS uploads
                                     │  /health/*       │◄── K8s probes
                                     └─────────────────┘
```

## Tech Stack

| Layer          | Technology                                                              |
| -------------- | ----------------------------------------------------------------------- |
| Runtime        | Bun                                                                     |
| HTTP Framework | Hono                                                                    |
| GraphQL        | PostGraphile v4 (auto-generated from PostgreSQL), served via graphql-yoga |
| ORM            | Drizzle (schema definitions + migrations)                               |
| Database       | PostgreSQL (via PgBouncer)                                              |
| Search         | OpenSearch (optional)                                                   |
| Effect system  | Effect-TS (typed errors, tracing, structured concurrency)               |
| Telemetry      | OpenTelemetry + Sentry                                                  |
| API docs       | OpenAPI / Swagger UI (auto-generated from hono-openapi)                 |

## Database Schema

The schema is defined in `api/src/services/storage/schema.ts` using Drizzle ORM. Migrations are generated with `drizzle-kit generate` and live in `api/drizzle/` as raw SQL files.

### Knowledge Graph Tables

The core data model is an entity-attribute-value (EAV) graph with space scoping:

- **`entities`** — Nodes in the knowledge graph. UUID primary key, with created/updated timestamps and block numbers.
- **`values`** — Property values for entities. Each row stores a single property value for an entity within a space. Has type-specific columns for every GRC-20 v2 data type (boolean, integer, float, decimal, text, bytes, date, time, datetime, schedule, point, rect, embedding) plus metadata (language, unit) and UTC-normalized columns for time queries.
- **`relations`** — Edges in the knowledge graph. Links entities with a typed, directed relationship scoped to a space. Fields: entity_id, type_id, from_entity_id, to_entity_id, position, space_id, verified.

### Space and Membership Tables

- **`spaces`** — DAO or Personal spaces with a contract address and optional topic reference.
- **`members`** / **`editors`** — Space membership via composite primary key (memberSpaceId, spaceId).
- **`subspaces`** — Space hierarchy with verified/related types.
- **`subspace_topics`** — Topic associations for subspaces.

### Governance Tables

- **`proposals`** — Governance proposals with voting mode (Fast/Slow), quorum, threshold, timing, and denormalized vote counts (yesCount, noCount, abstainCount).
- **`proposal_actions`** — Actions within a proposal. Discriminated union by action_type: AddMember, RemoveMember, AddEditor, RemoveEditor, Publish, Flag, Unflag, UpdateVotingSettings, and subspace actions.
- **`proposal_votes`** — Per-proposal votes with composite PK (proposalId, voterId).
- **`proposal_tally_queue`** — Async queue for background vote count recomputation. Vote writes enqueue a tally job; a sidecar worker updates the denormalized counts.

### Scoring Tables

- **`global_scores`** / **`local_scores`** / **`space_scores`** — Entity and space scoring at different scopes.
- **`votes`** / **`user_votes`** / **`votes_count`** — Curation voting with pre-aggregated counts.

### Versioned Tables (Temporal)

Used for entity history and diff computation:

- **`edit_versions`** — Maps edit_id → version_key (packed bigint: `block_number << 32 | sequence`). This encoding enables efficient range queries on a single indexed column.
- **`value_versions`** — Temporal value snapshots with valid_from_key/valid_to_key range columns. When a value changes, the current row's valid_to_key is set and a new row is inserted.
- **`relation_versions`** — Same temporal pattern for relations.

### Shared Tables

- **`ipfs_cache`** — IPFS content cached by the indexer pipeline. Used by the API's proposal-diff endpoint to decode edit blobs without network calls.
- **`meta`** — Cursor tracking per indexer.
- **`atlas_checkpoints`** — Atlas canonical graph state for restart recovery.

## Query Patterns

### PostGraphile GraphQL (`/graphql`)

PostGraphile auto-generates a full GraphQL schema from the PostgreSQL `public` schema. It's configured as read-only (all mutations disabled).

Key configuration:

- **Custom plugins** for UUID handling (dashed and undashed), value scalars (GeoPoint, GeoRect, Date), and efficient space/type filtering via EXISTS subqueries
- **Connection filter plugin** for rich filtering (is, isNot, in, notIn, etc.)
- **Separate pg pool** (max 50 connections) since PostGraphile holds connections for the duration of GraphQL resolution
- No JWT auth or transactions — the pool lifecycle is managed directly for simplicity

PostGraphile serves general-purpose knowledge graph queries: entity lookups, relation traversals, value filtering, space membership, etc.

### Custom REST Endpoints

REST endpoints exist for use cases where PostGraphile is insufficient — either because the query is too complex, requires computation, or involves data outside the knowledge graph tables.

All REST handlers use Effect-TS generators with tagged errors (ValidationError, NotFoundError, QueryError) and return structured responses via `Either.match`.

| Route                                             | Method | Purpose                                    |
| ------------------------------------------------- | ------ | ------------------------------------------ |
| `/versioned/entities/:id`                         | GET    | Entity snapshot at a specific version      |
| `/versioned/entities/:id/versions`                | GET    | List versions (edits) for an entity        |
| `/versioned/entities/:id/diff`                    | GET    | Diff between two versions of an entity     |
| `/versioned/proposals/:id/diff`                   | GET    | Proposal diff (paginated)                  |
| `/proposals/:id/status`                           | GET    | Single proposal status with vote counts    |
| `/proposals/space/:spaceId/status`                | GET    | List proposals (cursor pagination, filtering) |
| `/proposals/space/:spaceId/members/:id/active`    | GET    | Active ADD_MEMBER proposal check           |
| `/proposals/space/:spaceId/editors/:id/active`    | GET    | Active ADD_EDITOR proposal check           |
| `/profile/address/:address`                       | GET    | Profile by wallet address                  |
| `/profile/space/:spaceId`                         | GET    | Profile by space ID                        |
| `/profile/batch`                                  | POST   | Batch profile fetch (up to 100)            |
| `/search`                                         | GET    | Full-text search (OpenSearch)              |
| `/ipfs/upload-edit`                               | POST   | Upload edit to IPFS                        |
| `/ipfs/upload-file`                               | POST   | Upload file to IPFS                        |

REST queries use Drizzle's `sql` tagged template for raw SQL through a separate, smaller pool (max 18 connections).

## Connection Pools

The API maintains two separate PostgreSQL connection pools to prevent one query path from starving the other:

| Pool        | Max Connections | Used By                        |
| ----------- | --------------- | ------------------------------ |
| PostGraphile | 50              | GraphQL resolution             |
| Drizzle     | 18              | REST endpoints (versioned, proposals, profiles) |

Both pools have 3-second connection timeouts and route through PgBouncer.

## Health Checks

Kubernetes-aware probes:

- **Liveness** — Event loop alive, no I/O
- **Readiness** — Database reachable, no sustained pool saturation (tracked via a custom saturation detector that monitors recent acquire timeouts)
- **Pool metrics** — Exposes connection pool statistics

## Key Architectural Decisions

1. **Read-only API** — All mutations are disabled. Data only enters through the Rust indexer pipeline. The API is a pure read layer.

2. **Dual query paradigm** — PostGraphile for general-purpose graph queries (auto-generated, flexible), REST for specialized use cases (governance status computation, temporal diffs, search). We're considering making PostGraphile legacy in favor of more custom endpoints for better performance and consumer ergonomics.

3. **Effect-TS for business logic** — All REST handlers use Effect generators for typed error handling, structured concurrency, and tracing. This provides exhaustive error handling and correlates traces with business operations.

4. **Temporal versioning via packed bigint** — `version_key = (block_number << 32) | sequence` enables efficient range queries on a single indexed column for temporal lookups.

5. **Denormalized vote counts** — Proposal vote tallies are updated asynchronously via a queue to decouple the write path from tally computation.

6. **Search is optional** — If `OPENSEARCH_URL` is not set, search routes aren't mounted. The search indexer is independent.

7. **Two connection pools** — PostGraphile and REST endpoints use separate pools to prevent mutual starvation.

## Related Documents

- [Hermes Architecture](./architecture.md) — Blockchain ingestion pipeline (upstream of the API)
- [Gotchas](./gotchas.md) — Operational knowledge including indexer performance and API tradeoffs
- [Known Issues](./issues.md) — p99 query latency, PostGraphile vs REST decisions
- [Staging & Production](./runbooks/staging-production.md) — Deployment runbook
