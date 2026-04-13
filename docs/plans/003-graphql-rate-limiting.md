# Plan 003 — GraphQL HTTP Rate Limiting

## Goal

Protect the GraphQL HTTP endpoint from abuse (scrapers, runaway clients, accidental DOS) without throttling our own backend services or specific high-traffic clients we want to allow.

## Three tiers

Tier resolution is first-match-wins:

1. **Unlimited allowlist** (env CIDRs) — unlimited, counter not incremented, no rate-limit headers. Used for cluster-internal traffic and our own backend services.
2. **Per-IP override** (DB row) — uses the row's `requests_per_min`. Used for specific clients we've negotiated higher (or lower) limits with.
3. **Default** — `RATE_LIMIT_DEFAULT_PER_MINUTE` from env (1000/min). Applies to everyone else.

A per-IP override of `0` blocks the IP entirely — handy admin kill switch.

## Counter store: Valkey, fixed minute window

- Same Valkey instance as the response cache (`api-cache` in cluster), separate keyspace prefix `rl:`.
- Key per `(ip, unix_minute)`; `INCR` + `EXPIRE 120s` pipelined into a single round-trip.
- Counters are **shared across all API pods**, so a client cannot bypass the limit by hitting different pods.
- Fixed window has a known boundary effect (a client could burst up to 2× in a window straddling the minute edge). Acceptable for a soft guard. We can switch to a token bucket or true sliding window later if needed.

## Failure modes

| Failure | Behavior |
|---|---|
| Valkey down / slow / timeout | **Fail open** — allow request, log warn. Same posture as the response cache. Rate limiting is a soft guard, not a security boundary; an outage shouldn't take the API down. |
| Postgres down (override lookup) | Treat as "no override" → fall through to default. Don't cache the failure. |
| No `X-Forwarded-For` header | Allow request, log warn. Indicates ingress misconfiguration. |
| Limit env vars invalid | Fall back to defaults silently (logged). |

## DB schema

```sql
CREATE TABLE rate_limit_overrides (
  ip_range         cidr PRIMARY KEY,           -- '1.2.3.4' (auto /32) or '10.0.0.0/24'
  requests_per_min integer NOT NULL CHECK (requests_per_min >= 0),
  description      text,                       -- 'client: acme corp'
  created_at       timestamptz NOT NULL DEFAULT now(),
  updated_at       timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_rate_limit_overrides_ip_range
  ON rate_limit_overrides USING gist (ip_range inet_ops);
```

Lookup query:
```sql
SELECT requests_per_min FROM rate_limit_overrides
WHERE ip_range >>= $1::inet
ORDER BY masklen(ip_range) DESC
LIMIT 1;
```

Most-specific match wins (e.g. an explicit `/32` override beats a containing `/16`).

Per-pod in-memory cache with 60s TTL (`RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS`) keeps the hot path off Postgres. Both hits and misses are cached; trade-off is up to 60s for a new override to propagate to all pods.

## HTTP behavior

Allowed requests (response from upstream handler) include rate-limit headers:
```
RateLimit-Limit: 1000
RateLimit-Remaining: 957
RateLimit-Reset: 23
```

Blocked requests:
```
HTTP/1.1 429 Too Many Requests
RateLimit-Limit: 1000
RateLimit-Remaining: 0
RateLimit-Reset: 23
Retry-After: 23
Content-Type: application/json

{"error":"rate_limit_exceeded","retry_after_seconds":23}
```

Allowlisted IPs get **no headers** at all (signaling unlimited).

## Configuration

```bash
RATE_LIMIT_ENABLED=true
RATE_LIMIT_DEFAULT_PER_MINUTE=1000
RATE_LIMIT_UNLIMITED_ALLOWLIST_IPS=10.108.0.0/16,10.109.0.0/16   # DOKS pod + service nets
RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS=60
RATE_LIMIT_TRUSTED_PROXY_HOPS=1                         # ingress-nginx is 1 hop
```

Unlimited allowlist defaults to **DOKS pod CIDR (`10.108.0.0/16`)** and **service CIDR (`10.109.0.0/16`)** so all internal cluster traffic (pod-to-pod calls from notification-service, scoring-service, etc.) is exempted.

## Sizing rationale (1000/min default)

Frontend (geobrowser) makes ~10–20 GraphQL HTTP calls per page load (no batching link in `graphql-request`):

| User behavior | calls/min | vs 1000 limit |
|---|---|---|
| Casual: 5 page loads/min | ~50–100 | 5–10% |
| Heavy: 20 page loads/min | ~200–400 | 20–40% |
| Power: 50 page loads/min | ~500–1000 | 50–100% |
| Scraper crawling | usually >1000 | over (intended) |

1000/min ≈ 17 req/sec average — generous for any human, tight enough to trip naive scrapers.

Shared egress IPs (corporate NAT, mobile carriers) may bunch many users behind one address. The override table is the pressure-release valve — add a row for a known shared IP with a higher limit.

## Scope

- **v1**: applied to `/graphql` only.
- **Future**: easy to extend to `/search`, `/proposals`, etc. by mounting the same middleware.

## File layout

```
api/src/middleware/rateLimit.ts            # Hono middleware, ties everything together
api/src/middleware/clientIp.ts             # XFF parser with trusted-hops handling
api/src/services/rateLimit/cidr.ts         # IPv4 CIDR matcher
api/src/services/rateLimit/config.ts       # env → RateLimitConfig
api/src/services/rateLimit/store.ts        # Valkey INCR wrapper
api/src/services/rateLimit/overrides.ts    # Postgres lookup with per-pod cache
api/src/services/storage/schema.ts         # rate_limit_overrides table
api/drizzle/0054_curved_post.sql           # generated migration
api/main.ts                                # wires middleware before /graphql handler
api/.env.example                           # documents new env vars
```

## Operations

### Adding a per-IP override

```sql
-- Specific client gets 5000/min
INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
VALUES ('203.0.113.42', 5000, 'client: acme corp');

-- Whole subnet of an enterprise customer gets 10000/min
INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
VALUES ('198.51.100.0/24', 10000, 'enterprise: globex office');

-- Block a misbehaving IP entirely
INSERT INTO rate_limit_overrides (ip_range, requests_per_min, description)
VALUES ('192.0.2.99', 0, 'blocked: scraping');

-- Update an existing override
UPDATE rate_limit_overrides
SET requests_per_min = 2000, updated_at = now()
WHERE ip_range = '203.0.113.42';

-- Remove an override (IP falls back to default)
DELETE FROM rate_limit_overrides WHERE ip_range = '203.0.113.42';
```

Overrides take effect within `RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS` (default 60) on each pod.

### Verifying it's working

```bash
# Hit /graphql repeatedly, watch headers
for i in $(seq 1 5); do
  curl -sI -X POST -H 'Content-Type: application/json' \
    -d '{"query":"{__typename}"}' \
    https://api.geobrowser.io/graphql \
    | grep -i ratelimit
done
```

### Disabling temporarily

```bash
kubectl -n api set env deployment/api RATE_LIMIT_ENABLED=false
# revert with:
kubectl -n api set env deployment/api RATE_LIMIT_ENABLED=true
```

## Future work

- IPv6 support in `cidr.ts` (Postgres handles it natively in the table already).
- Token bucket or true sliding window if minute-edge boundary effect causes complaints.
- API key support: `Authorization: Bearer <key>` mapped to a client ID, override table keyed by client ID instead of (or in addition to) IP.
- Per-route rate limits with different defaults (e.g. tighter on `/ipfs/upload-*`).
- Prometheus metric: `graphql_rate_limit_total{status="allowed|blocked|allowlisted"}`.
