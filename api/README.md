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

## Rate Limits

The `/graphql` endpoint applies a per-IP rate limit (default **1000 requests per minute**) to protect against scraping and runaway clients. Counters are shared across all API pods.

Responses include standard rate-limit headers:

```
RateLimit-Limit: 1000
RateLimit-Remaining: 957
RateLimit-Reset: 23
```

When a client exceeds the limit, the API returns `HTTP 429 Too Many Requests` with `Retry-After` indicating seconds until the window resets:

```json
{"error":"rate_limit_exceeded","retry_after_seconds":23}
```

**Cluster-internal traffic** (pod-to-pod calls within DOKS) is exempt from rate limiting.

**API keys**: backend services (e.g. Railway-hosted curator-app) can send an `X-Api-Key` header to use a key-based limit instead of IP-based. Keys are stored in the `api_keys` table with either a custom `requests_per_min` or `NULL` for unlimited. Counters are tracked separately per key, independent of IP counters.

**Need a higher limit?** If you are a partner or integrator and 1000 req/min is not enough for your use case, **contact the Geo team** (open an issue at [geobrowser/gaia/issues](https://github.com/geobrowser/gaia/issues) or reach out via the project's communication channels) to request an API key or an override for your IP range.

**Capacity**: each API pod tracks up to **100,000 distinct IPs** in its in-memory override cache (LRU). Worst-case memory impact: ~20 MB per pod. Beyond 100k unique IPs in the active window, the least-recently-seen IP's override is re-fetched from Postgres on its next request.

**Identifying the client IP**: `RATE_LIMIT_TRUSTED_PROXY_HOPS` (default `1`) tells the limiter how many proxies we own sit in front of the API pod, so it can pick the correct entry from `X-Forwarded-For`. For our DOKS setup the only trusted hop is ingress-nginx, hence `1`. Bump this if you ever add a CDN (e.g. Cloudflare → `2`); without the right value, clients could either spoof their IP via `X-Forwarded-For` or get bucketed into the wrong counter.

### Disabling rate limiting

Rate limiting auto-disables when `RATE_LIMIT_ENABLED=false` **or** when `VALKEY_URL` is unset. Local dev (no Valkey) and CI/CD load tests are unaffected by default.

To disable in a running cluster (e.g. for a load test):

```bash
kubectl -n api set env deployment/api RATE_LIMIT_ENABLED=false
# revert with:
kubectl -n api set env deployment/api RATE_LIMIT_ENABLED=true
```

See [`docs/plans/003-graphql-rate-limiting.md`](../docs/plans/003-graphql-rate-limiting.md) for the full design.

## Documentation

- [API Architecture](../docs/api-architecture.md) — layers, tech stack, query patterns
- [Database Configuration](docs/database-configuration.md) — PostgreSQL and PgBouncer setup
- [Search Query Architecture](src/services/search/QUERY_ARCHITECTURE.md) — OpenSearch query design
