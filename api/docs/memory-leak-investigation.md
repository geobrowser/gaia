# Memory Leak Investigation

Ongoing investigation into gradual memory growth in the API pods. Memory climbs with traffic and does not return to baseline — only a deploy (process restart) resets it.

## Observed Behavior

- Fresh pods start at ~230–280 MiB
- Memory climbs monotonically: 280 → 330 → 430 → 488 → 507 MiB over hours
- Traffic bursts accelerate growth, but memory does not recede after bursts
- Growth rate was ~25 MiB/hour before initial fixes, now slower but still present
- No OOM kills or restarts observed — pods stay well within the 1 GiB limit
- Deploys reset memory to baseline (new process, fresh heap)

## Timeline of Fixes

### 1. Connection pool exhaustion — `5d88b96` (~Jan 28)

**Problem:** Requests hung for 290s+ because the pg pool was too small (10) with no connection timeout.

**Fix:** Bumped pool to 50, added 3s connection timeout.

**Impact:** Not a memory leak, but hung requests held their entire context in memory indefinitely. Fixing this reduced memory pressure from queued requests.

### 2. Unbounded caches + watchPg — `97f55a1` (~Feb 6)

**Problem:** Two culprits found:
- `watchPg` was accumulating schema snapshots via LISTEN/NOTIFY in production (dev-only feature that should never have been enabled)
- Response cache had `max: Infinity`

**Fix:** Disabled `watchPg`, capped response cache at 1024 entries with LRU eviction.

**Impact:** Eliminated the two largest known sources of unbounded growth. Memory growth rate dropped from ~25 MiB/hour.

### 3. CPU throttling cascade — `d373244` (~Feb 9)

**Problem:** CPU limit of 1000m caused 83% CFS throttling. The liveness probe hit Postgres, so throttled pods failed health checks and got killed/restarted.

**Fix:** Raised CPU limit to 2000m, added a no-I/O liveness endpoint.

**Impact:** Not a memory leak, but the restart cascade masked memory trends by constantly resetting pod memory. With stable pods, the underlying leak became visible.

### 4. pgClient lifecycle bug — `8bdf493` (~Feb 9)

**Problem:** `withPostGraphileContext` was releasing the pg client back to the pool *before* resolvers ran. This existed since the first commit.

**Fix:** Replaced with a Yoga plugin (`usePgClient`) that holds the client through execution and releases in `onExecuteDone`.

**Impact:** Could have caused leaked connections that indirectly grew memory. Also a correctness fix — resolvers were running against a potentially-reused client.

### 5. Response cache dispose bug — `1d46170` (~Feb 10–11)

**Problem:** `@envelop/response-cache`'s `createInMemoryCache` has a bug: `lru-cache` v10 calls `dispose(value, key, reason)` but the callback treated the first arg as the key. The entity-tracking Maps (`entityToResponseIds`, `responseIdToEntityIds`) were **never cleaned up on eviction**, growing unboundedly with query diversity.

**Fix:** Replaced with a simple LRU cache (`createSimpleResponseCache`) that only does TTL-based expiry. We don't use entity-based invalidation (mutations are disabled), so the side Maps were unnecessary.

**Impact:** Eliminated the largest remaining source of unbounded growth. This was the single most impactful fix.

### 6. Compression offload — `96b03d6` (~Feb 10–11)

**Problem:** Response compression (gzip/brotli) was running inside the Bun process, allocating per-request compression buffers.

**Fix:** Moved compression to nginx ingress.

**Impact:** Reduced per-request memory allocation in the API process.

### 7. LRU cache dependency — `ae2ade6` (~Feb 10–11)

**Housekeeping:** Made `lru-cache` an explicit dependency instead of transitive.

## Ruled Out

Components investigated and confirmed **not leaking**:

| Component | Why it's clean |
|-----------|---------------|
| PostGraphile schema | Created once at startup with `watchPg: false`. Fixed cost, does not grow. |
| Effect ManagedRuntime | Created once. Each `runPromise()` creates a Fiber that's collected on completion. |
| LRU response cache | Custom `createSimpleResponseCache` bounded at 1024 entries. No entity-tracking side Maps. |
| Proposal diff deserialization | `decodeEditAuto` creates large temp allocations (decoded ops, dictionaries, Uint8Arrays) but all references are local to the Effect generator. Eligible for GC when the request completes. |
| Sentry breadcrumbs | Hard-capped at 100 per scope, FIFO eviction. Per-request isolation via `AsyncLocalStorage` — each request gets a cloned scope that's GC'd when the request finishes. |
| Sentry event buffer | `PromiseBuffer` with max 64 concurrent sends. When full, events are dropped immediately, not queued. No unbounded accumulation. |
| OTEL SentrySpanProcessor | Time-bounded ring buffer (300 slots, 5-min TTL). Spans evict automatically. `_sentSpans` Map has TTL-based cleanup. This is a fixed-window cost, not a monotonic leak — memory would come back down if this were the cause. |
| Drizzle pg pool | No hung queries observed in DB metrics. Pool at max 18. `connectionTimeoutMillis: 3000` and `idleTimeoutMillis: 30000` now configured. |
| Bun runtime memory leaks | Production Docker image uses `oven/bun:1` which floats to latest 1.x — verified via Docker Hub digest that `:1` = `:1.3.9` as of Feb 8 2026. Bun 1.3.6 fixed streaming response leaks in `Bun.serve()` and `fetch()`, 1.3.7 upgraded mimalloc v3. These fixes are already in production. Memory still grows, so the leak is application-level, not Bun runtime. |

## Current Architecture

```
Request → Bun HTTP → Hono middleware (requestId → cors → canonicalRequestLogging)
  ├─ /graphql → graphql-yoga → plugins → PostGraphile resolvers
  │              ├─ usePgClient (pgPool, max 50)
  │              ├─ useExecutionCancellation
  │              ├─ useResponseCache (LRU, max 1024, 10s TTL)
  │              └─ useGraphQLInstrumentation (OTEL spans)
  ├─ /versioned/* → Effect handlers (Drizzle, _pool max 18)
  ├─ /proposals/* → Effect handlers (Drizzle, _pool max 18)
  ├─ /profile/*   → Effect handlers (Drizzle, _pool max 18)
  ├─ /search/*    → OpenSearch client
  └─ /ipfs/*      → Effect handlers
```

Two separate pg pools:
- **`pgPool` in postgraphile.ts**: max 50, 3s connection timeout, 30s idle — GraphQL
- **`_pool` in storage.ts**: max 18, 3s connection timeout, 30s idle — REST/Drizzle

## Open Leads

### graphql-yoga document cache

Yoga parses and caches GraphQL documents internally. If there is high query diversity (many unique query strings), this cache could grow unboundedly. Need to check:
- Does yoga use an LRU or an unbounded Map for parsed documents?
- What's the cache key — the full query string?
- Is there a `documentCacheSize` option?

### PostGraphile query plan cache

PostGraphile v4 may cache parsed/validated queries internally. Need to check:
- Does `createPostGraphileSchema` result in any runtime caches?
- Are query plans cached per unique query, and is that bounded?

### graphql-js parse/validate caching

The `graphql` package itself may cache parse results. Need to verify whether yoga or PostGraphile layers add their own caching on top.

### Bun heap fragmentation

Even if all application-level leaks are fixed, Bun's JavaScript engine (JavaScriptCore) may not return freed heap pages to the OS. Large temporary allocations (like decoded edit blobs at 100s of KiB) cause the heap high-water mark to ratchet up. This would explain the "goes up during bursts, doesn't fully come back down" pattern without being a true leak.

Possible mitigations:
- Explicit GC hints after large allocations (if Bun exposes this)
- Streaming/chunked processing instead of materializing entire decoded edits
- Monitoring RSS vs heap used to distinguish fragmentation from true leaks

## Resolved Latent Risks

### Drizzle pool connection timeout — fixed

`_pool` in `storage.ts` previously had no `connectionTimeoutMillis` (defaults to 0 = wait forever). Added `connectionTimeoutMillis: 3000` and `idleTimeoutMillis: 30000` to match the PostGraphile pool config.

### Bun version pinning — fixed

Dockerfile used `oven/bun:1` (floating tag). CI workflows used various outdated versions (1.2.21, 1.3.8, `latest`). Pinned everything to `1.3.9` for reproducibility. Added `bun 1.3.9` to `api/.tool-versions`.

## Next Step: Heap Profiling

Since Bun runtime leaks are ruled out (production is already on 1.3.9), the next investigation step is heap profiling to identify what's accumulating.

### Heap profiling tools

**CLI flags (Bun 1.3.7+):**
- `bun --heap-prof run main.ts` — writes `.heapsnapshot` at exit (V8-compatible, Chrome DevTools → Memory tab)
- `bun --heap-prof-md run main.ts` — writes markdown report with type-by-retained-size tables, retainer chains

**Production snapshots via debug endpoint:**

The API has a `/debug/heap-snapshot` endpoint, gated behind `ENABLE_DEBUG_ENDPOINTS` env var. To use it:

1. Set `ENABLE_DEBUG_ENDPOINTS=1` on the deployment (via secret or env patch)
2. Take a snapshot from a running pod:
   ```bash
   POD=$(kubectl get pods -n api -l app=api -o jsonpath='{.items[0].metadata.name}')
   kubectl exec -n api $POD -- curl -s localhost:3000/debug/heap-snapshot
   # Returns: {"path":"/tmp/heap-<ts>.heapsnapshot","filename":"heap-<ts>.heapsnapshot"}
   kubectl cp api/$POD:/tmp/heap-<ts>.heapsnapshot ./heap-fresh.heapsnapshot
   ```
3. Wait several hours under traffic, take another snapshot, compare in Chrome DevTools

**What to look for:**
- Object types with monotonically increasing retained size
- String/Buffer accumulation (suggests cached query strings or response bodies)
- Map/Set instances with growing entry counts (suggests unbounded caches)
- Closures retaining request-scoped variables beyond request lifetime

**Key suspects to validate:**
- graphql-yoga document parse cache (bounded LRU? or unbounded Map?)
- PostGraphile query plan cache
- graphql-js internal caches
- Hono middleware closures or context objects
