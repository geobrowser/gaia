# Database Configuration

This document describes the PostgreSQL and PgBouncer configuration for the Geo API.

## Architecture

```
API Replicas (3x)              PgBouncer                 PostgreSQL
┌─────────────────┐           ┌─────────────┐           ┌─────────────┐
│ pg Pool: 50     │──────────▶│ max: 200    │──────────▶│ max: 100    │
│ per replica     │           │ pool: 70    │           │             │
└─────────────────┘           └─────────────┘           └─────────────┘
     150 total                  multiplexes               70 active
   client conns                to 70 server               connections
                                connections
```

## PostgreSQL Settings

| Setting | Value | Purpose |
|---------|-------|---------|
| `max_connections` | 100 | Maximum concurrent connections |
| `statement_timeout` | 10s | Kill queries running longer than 10s |
| `idle_in_transaction_session_timeout` | 30s | Kill connections stuck in a transaction without committing |

### Applying PostgreSQL Settings

```sql
-- Set idle transaction timeout (requires reload, not restart)
ALTER SYSTEM SET idle_in_transaction_session_timeout = '30s';
SELECT pg_reload_conf();

-- Verify
SELECT name, setting, unit
FROM pg_settings
WHERE name IN ('statement_timeout', 'idle_in_transaction_session_timeout', 'max_connections');
```

## PgBouncer Settings

| Setting | Value | Purpose |
|---------|-------|---------|
| `pool_mode` | transaction | Release connections after each transaction (most efficient) |
| `default_pool_size` | 70 | Max connections to PostgreSQL per database/user |
| `min_pool_size` | 50 | Connections kept warm for instant availability |
| `max_client_conn` | 200 | Max connections from applications to PgBouncer |
| `reserve_pool_size` | 10 | Extra connections for traffic spikes |
| `query_timeout` | 15 | Kill queries running longer than 15s (seconds) |
| `idle_transaction_timeout` | 30 | Kill idle transactions after 30s (seconds) |
| `server_idle_timeout` | 600 | Close idle PostgreSQL connections after 10 min |

### Applying PgBouncer Settings

Connect to the `pgbouncer` database and run SET commands:

```sql
-- Connect to PgBouncer admin
psql "postgres://user:pass@pgbouncer-host:port/pgbouncer"

-- Apply settings
SET default_pool_size = 70;
SET min_pool_size = 50;
SET max_client_conn = 200;
SET reserve_pool_size = 10;
SET query_timeout = 15;
SET idle_transaction_timeout = 30;

-- Verify
SHOW config;
```

> **Note**: These settings are in-memory. To persist across restarts, update the PgBouncer config file (`pgbouncer.ini`).

### Monitoring PgBouncer

```sql
-- Check pool status
SHOW pools;

-- Key metrics:
-- cl_active: active client connections
-- cl_waiting: clients waiting for a connection (should be 0)
-- sv_active: server connections running queries
-- sv_idle: server connections ready to serve

-- Check connected clients
SHOW clients;

-- Check server connections
SHOW servers;
```

## Node.js pg Pool Settings (API)

Configured in `src/kg/postgraphile.ts`:

| Setting | Value | Purpose |
|---------|-------|---------|
| `max` | 50 | Connections per replica to PgBouncer |
| `connectionTimeoutMillis` | 3000 | Fail fast if no connection available (3s) |
| `idleTimeoutMillis` | 30000 | Close idle connections after 30s |
| `allowExitOnIdle` | true | Allow graceful shutdown |

### Environment Variables

All settings can be overridden via environment variables:

```bash
PG_POOL_MAX=50
PG_CONNECTION_TIMEOUT_MS=3000
PG_IDLE_TIMEOUT_MS=30000
```

## Capacity Planning

### Connection Math

```
API replicas × pool size = client connections to PgBouncer
3 replicas × 50 = 150 client connections

PgBouncer max_client_conn = 200 (headroom for scaling to 4 replicas)
PgBouncer default_pool_size = 70 (connections to PostgreSQL)
PostgreSQL max_connections = 100 (30 reserved for indexers/admin/emergencies)
```

## Logging

### Slow Query Logging

Enable logging for queries taking longer than a threshold:

```sql
-- Log queries taking longer than 1 second
ALTER SYSTEM SET log_min_duration_statement = 1000;
SELECT pg_reload_conf();

-- Verify
SELECT name, setting FROM pg_settings WHERE name = 'log_min_duration_statement';
```

| Value | Behavior |
|-------|----------|
| `-1` | Disabled (default) |
| `0` | Log all queries |
| `1000` | Log queries > 1 second |

### Query Statistics (pg_stat_statements)

For historical query performance data, `pg_stat_statements` must be loaded via `shared_preload_libraries` (requires PostgreSQL restart).

```sql
-- Check if available
SELECT * FROM pg_extension WHERE extname = 'pg_stat_statements';

-- If loaded, query top slow queries
SELECT
  calls,
  round(total_exec_time::numeric, 0) AS total_ms,
  round(mean_exec_time::numeric, 0) AS avg_ms,
  round(max_exec_time::numeric, 0) AS max_ms,
  left(query, 100) AS query
FROM pg_stat_statements
ORDER BY mean_exec_time DESC
LIMIT 15;
```

## Phase 2 Offender Query Pack

Use this query pack weekly to identify slow SQL offenders by both total impact and tail behavior.

### 1) Top queries by total DB time (primary ranking)

```sql
SELECT
  queryid,
  calls,
  round(total_exec_time::numeric, 0) AS total_ms,
  round(mean_exec_time::numeric, 2) AS mean_ms,
  round(max_exec_time::numeric, 2) AS max_ms,
  round((total_exec_time / NULLIF(calls, 0))::numeric, 2) AS avg_ms,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 25;
```

### 2) Tail outliers (high max latency with enough volume)

```sql
SELECT
  queryid,
  calls,
  round(mean_exec_time::numeric, 2) AS mean_ms,
  round(max_exec_time::numeric, 2) AS max_ms,
  round(total_exec_time::numeric, 0) AS total_ms,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_statements
WHERE calls >= 50
ORDER BY max_exec_time DESC
LIMIT 25;
```

### 3) Lock-sensitive offenders (often hidden by averages)

```sql
SELECT
  pid,
  now() - query_start AS age,
  wait_event_type,
  wait_event,
  state,
  left(regexp_replace(query, '\\s+', ' ', 'g'), 180) AS query
FROM pg_stat_activity
WHERE datname = current_database()
  AND state = 'active'
  AND wait_event_type = 'Lock'
ORDER BY age DESC
LIMIT 25;
```

### 4) Correlate offenders to API operations

Use `queryid` and statement snippet from the queries above with API/Sentry context:

- GraphQL operation names and request IDs are logged in:
  - `api/src/kg/postgraphile.ts`
  - `api/src/middleware/requestLogging.ts`
- Failure-class tags in Sentry include:
  - `db.failure_class`
  - `graphql.operation_name`

Recommended triage workflow:

1. Rank top 10 by `total_exec_time`.
2. For each offender, collect top GraphQL operation names/request IDs in the same time window.
3. Run `EXPLAIN (ANALYZE, BUFFERS)` for the highest-impact 3 offenders.
4. Prioritize fixes by total DB time share first, then tail (`max_exec_time`).

### 5) Weekly output template (copy into incident or planning docs)

| Rank | queryid | total_ms | calls | mean_ms | max_ms | Operation(s) | Owner | Action |
|------|---------|----------|-------|---------|--------|--------------|-------|--------|
| 1 | ... | ... | ... | ... | ... | ... | ... | Index/query rewrite |
| 2 | ... | ... | ... | ... | ... | ... | ... | Pagination/shape limit |
| 3 | ... | ... | ... | ... | ... | ... | ... | Plan analysis + migration |

### Grafana support notes

- Current Grafana/Prometheus setup in this repo tracks ingress and route-level pressure proxies.
- Use dashboard panels to quickly narrow suspect routes, then run SQL query pack commands above for true SQL offenders.
- If you want direct SQL-fingerprint panels in Grafana, add PostgreSQL exporter metrics for `pg_stat_statements` and create recording rules keyed by `queryid`.

## Troubleshooting

### Check for Connection Issues

```sql
-- PostgreSQL: Check active connections
SELECT
  count(*) AS total,
  count(*) FILTER (WHERE state = 'active') AS active,
  count(*) FILTER (WHERE state = 'idle') AS idle,
  count(*) FILTER (WHERE state = 'idle in transaction') AS idle_in_txn
FROM pg_stat_activity;

-- PostgreSQL: Check for blocked queries
SELECT pid, state, wait_event_type, query
FROM pg_stat_activity
WHERE wait_event_type = 'Lock';

-- PgBouncer: Check for waiting clients
SHOW pools;
-- Look for cl_waiting > 0
```

### Common Issues

| Symptom | Likely Cause | Solution |
|---------|--------------|----------|
| `cl_waiting > 0` in PgBouncer | Pool exhausted | Increase `default_pool_size` or optimize slow queries |
| `idle in transaction` connections | App not committing | Check for missing commits; `idle_in_transaction_session_timeout` will kill these |
| "too many connections" in PostgreSQL | PgBouncer pool too large | Reduce `default_pool_size` |
| 502 errors during deploy | New replicas not ready | Add readiness probes; connections refused during startup |
