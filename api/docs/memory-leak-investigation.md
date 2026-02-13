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
| Bun runtime memory leaks | Dockerfile previously used `oven/bun:1` (floating tag), now pinned to `oven/bun:1.3.9`. Verified via Docker Hub digest that production was already on 1.3.9 before pinning. Bun 1.3.6 fixed streaming response leaks in `Bun.serve()` and `fetch()`, 1.3.7 upgraded mimalloc v3. Memory still grows, so the leak is application-level, not Bun runtime. |

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

### graphql-yoga parser/validation cache (confirmed bounded)

Yoga installs `useParserAndValidationCache` by default (unless `parserAndValidationCache: false`). It caches parsed documents and parse errors using an LRU cache.

- **Cache key:** full query string (`params.source.toString()`).
- **Default size:** max 1024 entries, TTL 1 hour.
- **Bounded:** yes, LRU + TTL; high query diversity can fill it but it should not grow unboundedly.

If needed, this can be disabled or configured via `parserAndValidationCache` in `createYoga`.

### PostGraphile query cache (bounded, likely not used here)

PostGraphile’s HTTP handler has a bounded LRU cache for parsed/validated queries (default `queryCacheMaxSize` = 50 MiB, effectively ~525 entries). Only queries <100 KB are cached. The cache resets when the schema changes.

However, this cache is part of PostGraphile’s HTTP handler and is **not used** in our Yoga integration (we only use `createPostGraphileSchema`).

### graphql-js parse/validate caching

The `graphql` package itself does **not** maintain a global parse/validate cache. `parse()` and `validate()` construct new parser/context instances per call. Some validation rules (e.g., `OverlappingFieldsCanBeMergedRule`) maintain **per-validation** Maps to memoize work, but these are scoped to a single validation run and should be GC’d after the request completes.

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

1. Set `ENABLE_DEBUG_ENDPOINTS=1` on the deployment (currently enabled in both staging and production)
2. Take a snapshot from a running pod (no `curl` in the container — use `bun -e` with `fetch`):
   ```bash
   POD=$(kubectl get pods -n api -l app=api -o jsonpath='{.items[0].metadata.name}')
   kubectl exec -n api $POD -- bun -e "
     const res = await fetch('http://localhost:3000/debug/heap-snapshot');
     console.log(await res.text());
   "
   # Returns: {"path":"/tmp/heap-<ts>.heapsnapshot","filename":"heap-<ts>.heapsnapshot"}
   kubectl cp api/$POD:/tmp/heap-<ts>.heapsnapshot ./heap.heapsnapshot
   ```
3. Wait several hours under traffic, take another snapshot, compare using the script above

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

### Baseline heap snapshot — Feb 13, 2026

Taken from a fresh pod (~minutes after deploy, pod `api-7bbdb8ccb4-6clzg`, working set ~288 MiB).

**Note on snapshot format:** `Bun.generateHeapSnapshot()` produces JSC Inspector format (`"type":"Inspector"`), not V8 format. These files don't load in Chrome DevTools. Parse them programmatically — the node layout is 4 fields per node where field[2] is the `nodeClassNames` index.

**How to take a snapshot and parse it:**
```bash
POD=$(kubectl get pods -n api -l app=api -o jsonpath='{.items[0].metadata.name}')
kubectl exec -n api $POD -- bun -e "
const res = await fetch('http://localhost:3000/debug/heap-snapshot');
const data = await res.json();
const file = Bun.file(data.path);
const snap = await file.json();
const classNames = snap.nodeClassNames;
const nodes = snap.nodes;
const byClass = {};
for (let i = 0; i < nodes.length; i += 4) {
  const name = classNames[nodes[i + 2]] || '(unknown)';
  const size = nodes[i + 1];
  if (!byClass[name]) byClass[name] = { count: 0, totalSize: 0 };
  byClass[name].count++;
  byClass[name].totalSize += size;
}
Object.entries(byClass)
  .sort((a, b) => b[1].totalSize - a[1].totalSize)
  .slice(0, 20)
  .forEach(([name, s], i) => {
    const fmt = s.totalSize > 1024*1024
      ? (s.totalSize/1024/1024).toFixed(1)+' MiB'
      : (s.totalSize/1024).toFixed(1)+' KB';
    console.log((i+1) + '. ' + name + ': ' + s.count + ' instances, ' + fmt);
  });
"
```

**Baseline results (96.3 MiB JS heap, 288 MiB working set):**

| Rank | Type | Count | Size |
| ---: | ---- | ----: | ---: |
| 1 | `string` | 327,813 | 29.4 MiB |
| 2 | `ArrayBuffer` | 18 | 16.2 MiB |
| 3 | `ModuleRecord` | 2,199 | 10.4 MiB |
| 4 | `Object` | 141,161 | 8.9 MiB |
| 5 | `FunctionCodeBlock` | 2,420 | 6.5 MiB |
| 6 | `FunctionExecutable` | 30,091 | 3.7 MiB |
| 7 | `Structure` | 27,720 | 3.0 MiB |
| 8 | `Cell Butterfly` | 4,182 | 2.9 MiB |
| 9 | `UnlinkedFunctionExecutable` | 29,107 | 2.7 MiB |
| 10 | `Function` | 62,133 | 2.3 MiB |

**Key observations:**
- 96.3 MiB JS heap vs 288 MiB working set = ~192 MiB in native allocations (pg pools, TLS, Bun internals, mimalloc)
- `string` dominates (30% of heap) — expected for a server with many modules and query strings
- 141k `Object` instances and 62k `Function` instances — baseline for module loading
- 18 `ArrayBuffer`s account for 16.2 MiB — likely compiled bytecode or large buffers
- 6,598 `InternalPromise` instances — worth tracking; growth would indicate unresolved promises

**What to compare on next snapshot:**
- `string` count/size growing → cached query strings or response bodies
- `Object` count growing → accumulating request contexts or cache entries
- `InternalPromise` count growing → unresolved promises holding references
- `Array` count growing (47,909 baseline) → accumulating collections
- New types appearing in top 10 → new source of accumulation

### Follow-up snapshot — Feb 13, 2026 (same pod, ~420 MiB working set)

Same pod `6clzg`, after receiving traffic. Working set grew from 288 → 420 MiB (+132 MiB).

**JS heap comparison:**

| Type | Baseline Count | Now Count | Δ Count | Baseline Size | Now Size | Δ Size |
| ---- | -------------: | --------: | ------: | ------------: | -------: | -----: |
| `string` | 327,813 | 436,109 | +108,296 | 29.4 MiB | 38.6 MiB | +9.2 MiB |
| `Object` | 141,161 | 191,320 | +50,159 | 8.9 MiB | 12.1 MiB | +3.2 MiB |
| `Array` | 47,909 | 66,473 | +18,564 | 748 KB | 1.0 MiB | +290 KB |
| `FunctionCodeBlock` | 2,420 | 2,756 | +336 | 6.5 MiB | 7.4 MiB | +0.9 MiB |
| `ArrayBuffer` | 18 | 19 | +1 | 16.2 MiB | 16.2 MiB | ~0 |
| `ModuleRecord` | 2,199 | 2,199 | 0 | 10.4 MiB | 10.4 MiB | 0 |
| `Function` | 62,133 | 62,275 | +142 | 2.3 MiB | 2.3 MiB | ~0 |
| **Total JS heap** | | | **+180,944 nodes** | **96.3 MiB** | **110.1 MiB** | **+13.8 MiB** |

**Key finding: The leak is mostly native, not JS.**

Working set grew 132 MiB but JS heap only grew 13.8 MiB. ~90% of the memory growth (118 MiB) is in native allocations invisible to the JS heap profiler.

### Native memory analysis

Read from `/proc/1/status` and `/proc/1/smaps_rollup` on the running pod:

**Process memory breakdown (pod `6clzg` at 420 MiB working set):**

| Component | Size | Notes |
| --------- | ---: | ----- |
| VmRSS (total resident) | 421 MiB | What the OS reports |
| RssAnon (heap + stacks) | 367 MiB | All anonymous memory |
| RssFile (mmap'd files) | 54 MiB | Bun binary, shared libs — fixed cost |
| JS heap (from snapshot) | 110 MiB | Only 26% of RssAnon |
| Thread stacks (10 threads × 1 MiB) | ~10 MiB | Fixed cost |
| **Unaccounted native** | **~247 MiB** | **This is the leak** |
| VmHWM (peak RSS ever) | 956 MiB | Pod nearly hit the 1536 MiB limit at some point |

**Thread inventory (10 threads):**
- `bun` × 2 (main + event loop)
- `HeapHelper` × 3 (JSC GC helper threads)
- `Bun Pool` × 4 (Bun's thread pool for async I/O)
- `HTTP Client` × 1 (outbound HTTP connections)

**Comparison across pods (same age, different traffic):**

| | `6clzg` (more traffic) | `6jqw5` (less traffic) |
| --- | ---: | ---: |
| VmRSS | 421 MiB | 388 MiB |
| RssAnon | 367 MiB | 333 MiB |
| RssFile | 54 MiB | 54 MiB |
| VmHWM (peak) | 956 MiB | 640 MiB |

RssFile is identical (fixed). The difference is entirely in RssAnon (native heap), and it correlates with traffic volume.

### Revised understanding

The memory leak has two components:

1. **JS heap growth (~14 MiB):** Slow, mostly `string` (+108k instances, +9.2 MiB) and `Object` (+50k instances, +3.2 MiB). Likely graphql-yoga's document parse cache or PostGraphile internal caches accumulating unique query strings and parsed ASTs. Worth investigating but not the primary problem.

2. **Native memory growth (~118 MiB):** The dominant component. Not visible in JS heap snapshots. Candidates:
   - **mimalloc fragmentation** — Bun uses mimalloc v3. Large temporary allocations (decoded edit blobs in `/versioned/*/diff` can be 100s of KiB) fragment the arena. Pages get committed but never fully decommitted. The VmHWM of 956 MiB (spike then partial recovery to 420 MiB) is classic fragmentation — the allocator retains pages in its free lists even after the JS objects are GC'd.
   - **Bun HTTP server internals** — Each request through `Bun.serve()` allocates native request/response buffers. If not fully released on completion, they accumulate with request volume.
   - **pg driver TLS buffers** — Each pg connection through PgBouncer uses TLS. OpenSSL/BoringSSL contexts retain per-connection state that may not be fully freed on connection return to pool.
   - **Sentry/OTEL transport buffers** — Native HTTP client buffers for telemetry transport.

### Next steps

1. **Force GC + mimalloc purge** — Call `Bun.gc(true)` from a running pod and check if RssAnon drops. If it does, the "leak" is actually fragmentation/deferred decommit. If it doesn't, there's a true native allocation leak.
2. **Investigate JS string growth** — The +108k strings could be graphql-yoga's unbounded document cache. Check if yoga uses an LRU or a plain Map for parsed documents.
3. **Monitor with `--smol`** — Bun's `--smol` flag trades throughput for lower memory by using a smaller heap and more aggressive GC. Could mitigate fragmentation at the cost of CPU.
4. **Profile native allocations** — If `Bun.gc(true)` doesn't recover memory, use `jemalloc` or `heaptrack` to trace native allocation sites. This requires rebuilding the Docker image with profiling tools.
