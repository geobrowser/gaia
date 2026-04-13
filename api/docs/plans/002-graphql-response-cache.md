# Plan 002: GraphQL Response Cache

## Status

In progress

## Date

2026-04-08

## Problem

Production logs show repeated identical GraphQL queries producing large responses (2-15 MB) that hit the database every time. These are user-neutral queries (same result for all users) from server-side rendering in the geogenesis and curator-app frontends. The queries contribute to DB pool pressure and p95/p99 latency spikes, triggering unnecessary HPA replica scale-up.

### Observed in production (1 hour sample)

| Query | Count | Response Size | Duration |
|---|---|---|---|
| `entitiesOrderedByProperty` (inline, geogenesis) | 47 | 2.48 MB | ~1.1s |
| `entities` (1000 entities, geogenesis) | 10 | 14.95 MB | ~3-4s |
| `Spaces` (DAO spaces, curator-app) | 1-5 | 12.17 MB | ~1.8s |

All queries return identical bytes across calls — they are user-neutral and come from Next.js SSR (AWS us-east-1 IPs, user-agent `node`).

### Previous attempt

An in-memory response cache using `@graphql-yoga/plugin-response-cache` with a 1024-entry LRU was added and removed in PR #399. It caused OOM because serialized responses were stored in API pod memory alongside the application heap.

## Solution

Shared Valkey (Redis-compatible) cache deployed as a separate pod, used by all API pods via the Yoga response cache plugin.

### Architecture

```
┌─────────┐   ┌─────────┐   ┌─────────┐
│ API Pod │   │ API Pod │   │ API Pod │
│  1..N   │   │  1..N   │   │  1..N   │
└────┬────┘   └────┬────┘   └────┬────┘
     │              │              │
     └──────┬───────┴──────┬───────┘
            │              │
     ┌──────▼──────┐  ┌───▼────┐
     │   Valkey    │  │ PgPool │
     │  512MB LRU  │  │  → DB  │
     │  10/60s TTL │  │        │
     └─────────────┘  └────────┘
```

### Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Cache location | Dedicated Valkey pod, not in-memory | Previous in-memory cache caused OOM. Valkey isolates cache memory from API heap |
| Valkey vs Redis | Valkey 8.1 | BSD-3 license, drop-in Redis replacement, backed by AWS/Linux Foundation |
| TTL (default) | 10 seconds | Short enough for data freshness, long enough to absorb burst traffic |
| TTL (expensive queries) | 60 seconds | For spaces, entities, entitiesOrderedByProperty — near-static, user-neutral |
| Max memory | 512 MB with allkeys-lru | Conservative for ~15 entries at 25 MB each. LRU evicts when full |
| Pod memory limit | 1 Gi | 512 MB headroom above maxmemory for fragmentation/overhead |
| Session keying | `session: () => null` | Shared cache — queries keyed by query+variables, user-specific params produce distinct keys |
| Auth | Password via K8s Secret | Prevents cache poisoning from other pods in the cluster |
| Network | NetworkPolicy restricting ingress to API pods | Defense in depth alongside auth |
| Command timeout | 500 ms (ioredis commandTimeout) | If Valkey is slow/hanging, fall through to DB |
| Pool pressure shedding | Inside usePgClient, not before Yoga | Cache hits bypass shedding — cached responses served even when DB is saturated |
| Enabled/disabled | Optional via `VALKEY_URL` env var | No VALKEY_URL = no cache, zero impact |

### TTL per query type

| Schema Coordinate | TTL | Why |
|---|---|---|
| `Query.spaces` / `Query.spacesConnection` | 60s | DAO spaces list, 12 MB, near-static |
| `Query.entities` / `Query.entitiesConnection` | 60s | Bounties/entity lists, 2-15 MB |
| `Query.entitiesOrderedByProperty` | 60s | Ordered entity lists, repeated 47x/hour |
| Everything else | 10s | Default for less predictable queries |

### Trade-offs

- Cached responses may be up to 10-60 seconds stale depending on query type
- For the queries being cached, this is acceptable — they are public, read-heavy data
- Adds one more pod to operate (Valkey) — mitigated by ephemeral config (no persistence, no backups needed)
- Valkey being down = no cache, not an outage — graceful degradation

## Deployment

### Prerequisites

Create the cache secret (once per namespace):

```bash
# Production
kubectl create secret generic api-cache-secrets \
  --namespace=api \
  --from-literal=VALKEY_PASSWORD='<generate-a-strong-password>' \
  --from-literal=VALKEY_URL='redis://:<password>@api-cache.api.svc.cluster.local:6379'

# Staging
kubectl create secret generic api-cache-secrets \
  --namespace=api-staging \
  --from-literal=VALKEY_PASSWORD='<generate-a-strong-password>' \
  --from-literal=VALKEY_URL='redis://:<password>@api-cache.api-staging.svc.cluster.local:6379'
```

### Steps

1. Apply Valkey manifests: `kubectl apply -f api/k8s/production/redis.yaml`
2. Wait for Valkey pod: `kubectl get pods -n api -l app=api-cache`
3. Deploy updated API (picks up `VALKEY_URL` from secret)
4. Verify: `kubectl logs -l app=api -n api | grep "Response cache"`

### Rollback

Remove `VALKEY_URL` from the secret or delete the secret — cache is disabled, all requests go to DB.

### Monitoring

```bash
# Cache memory usage
kubectl exec -n api deploy/api-cache -- valkey-cli -a <password> --no-auth-warning info memory

# Cache hit/miss stats
kubectl exec -n api deploy/api-cache -- valkey-cli -a <password> --no-auth-warning info stats | grep keyspace

# Active keys
kubectl exec -n api deploy/api-cache -- valkey-cli -a <password> --no-auth-warning dbsize
```

## Files

| File | Description |
|---|---|
| `api/src/kg/valkeyCache.ts` | Valkey cache adapter for Yoga plugin |
| `api/src/kg/postgraphile.ts` | Cache plugin integration + pool shedding moved inside usePgClient |
| `api/main.ts` | Pre-fetch shedding removed, catch pool_pressure_shed error |
| `api/k8s/{production,staging}/redis.yaml` | Valkey deployment + service + NetworkPolicy |
| `api/k8s/{production,staging}/api.yaml` | VALKEY_URL from secret |
| `api/.env.example` | VALKEY_URL documented |

## PR

- geobrowser/gaia#564
- defi-wonderland/gaia#129
