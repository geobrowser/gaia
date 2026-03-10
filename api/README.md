# Gaia API

Read-only GraphQL and REST API serving the Geo knowledge graph, governance, versioning, search, and profile data.

## Overview

The API is a TypeScript service running on [Bun](https://bun.sh/) with [Hono](https://hono.dev/) as the HTTP framework. It serves two query paradigms:

1. **PostGraphile GraphQL** (`/graphql`) — auto-generated from the PostgreSQL schema
2. **Custom REST endpoints** — versioning, governance, profiles, search, IPFS uploads

All data enters through the Rust indexer pipeline (Kafka → indexers → PostgreSQL/OpenSearch). The API reads from PostgreSQL and OpenSearch.

```
Kafka Topics
    |
    +-- kg-indexer --------------> PostgreSQL
    +-- search-indexer ----------> OpenSearch
    +-- actions-indexer ---------> PostgreSQL
    +-- scoring-service ---------> PostgreSQL
                                        |
                                        v
                               +------------------+
                               |   Gaia API       |
                               |   (Bun + Hono)   |
                               |                  |
                               |  /graphql        |<-- PostGraphile
                               |  /versioned/*    |<-- Temporal entity queries
                               |  /proposals/*    |<-- Governance status
                               |  /profile/*      |<-- User profiles
                               |  /search/*       |<-- OpenSearch proxy
                               |  /ipfs/*         |<-- IPFS uploads
                               |  /health/*       |<-- K8s probes
                               +------------------+
```

## Local Development

### 1. Start PostgreSQL

```bash
docker compose up -d
```

This starts PostgreSQL on `localhost:5432` with user `postgres`, password `postgres`, database `gaia`.

### 2. Configure Environment

```bash
cp .env.example .env
# Edit .env with your values (see .env.example for descriptions)
```

### 3. Install Dependencies and Migrate

```bash
bun install
bun run db:migrate
```

### 4. Run

```bash
bun run start
```

The API starts on `http://localhost:3000` (default).

## Scripts

| Command | Description |
|---------|-------------|
| `bun run start` | Start the API server |
| `bun run test` | Run tests (vitest) |
| `bun run lint` | Lint with Biome |
| `bun run lint:fix` | Lint and auto-fix |
| `bun run format` | Format with Biome |
| `bun run check` | TypeScript type check |
| `bun run ci` | Format check + lint + type check |
| `bun run db:generate` | Generate Drizzle migrations |
| `bun run db:migrate` | Run database migrations |

## Configuration

See [`.env.example`](.env.example) for all environment variables with descriptions.

## Documentation

- [API Architecture](../docs/api-architecture.md) — layers, tech stack, query patterns
- [Database Configuration](docs/database-configuration.md) — PostgreSQL and PgBouncer setup
- [Search Query Architecture](src/services/search/QUERY_ARCHITECTURE.md) — OpenSearch query design
