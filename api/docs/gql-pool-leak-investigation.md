# GraphQL Pool Leak Investigation

Investigation into gradual GraphQL pool utilization growth that leads to 503s well before the pool reaches its configured limit. Companion to `memory-leak-investigation.md` — the two are independent issues but share triage tooling.

## Executive Summary

The GraphQL pg pool's utilization climbs monotonically over many hours under stable, low-volume traffic (5–30 rps), capping around 70% before pod restarts reset it. Once the pool crosses ~50% utilization, 503s start firing. Traffic volume does not justify the growth — the shape is a textbook **slow connection leak**, not a load curve.

The most likely root cause is the interaction between **`useExecutionCancellation()` and `usePgClient()`** in `api/src/kg/postgraphile.ts`: when a client cancels a request mid-flight (a 499), the GraphQL `execute()` call is aborted, but (a) `usePgClient`'s `onExecuteDone` cleanup is not guaranteed to fire on abort, and (b) even when it does, node-pg's `PoolClient` has no `AbortSignal` integration, so the in-flight SQL keeps running and `release()` on a busy client has undefined semantics in the pool. Either path leaves connections checked out and counted against pool capacity until they eventually time out (or never).

Several secondary issues amplify the impact: an aggressive saturation threshold (`PG_POOL_PRESSURE_WAITING_THRESHOLD=1` in production) trips 503s on momentary queues, missing nginx ingress timeouts and missing pg `query_timeout` mean the timeout hierarchy isn't fully wired end-to-end, and the pool metric currently exported (`totalCount` / `idleCount` separately) makes leaks harder to diagnose than a single "checked-out" gauge would.

This document records the findings and the recommended fix set, in priority order.

## Observed Behavior

From the production GraphQL pool dashboard (24-hour window):

- **Pool utilization** (`pgPool.totalCount / max`) starts each pod's life near 5% and grows steadily to ~70% over 12–14 hours, then plateaus.
- Two sharp drops to ~0% are visible (around 15:00 and 21:00) — these correspond to pod restarts.
- **Request volume** is stable at 5–30 rps for the same window, with no spikes that correlate to the utilization growth. Peak rps does not coincide with peak utilization.
- **5xx error rates** (mostly 503s) start ramping at ~04:00, exactly as utilization crosses ~50%.
- **499s (client cancels)** are present throughout, growing somewhat over the day.

The drops at restart followed by a fresh slow climb is the diagnostic shape — a load-driven utilization curve would track rps and oscillate, not climb monotonically across pod lifetimes.

## Why This Is a Leak, Not Load

`pgPool.totalCount` (numerator of the utilization metric — `api/src/services/dbSaturation.ts:80`, `api/src/kg/postgraphile.ts:64-71`) counts both idle and busy connections. The pool's behaviors are:

- **Idle connections** close after `idleTimeoutMillis = 30s` (`api/src/kg/postgraphile.ts:59`).
- **Busy connections** (`pool.connect()` succeeded, `release()` not yet called) never auto-close.

Under stable traffic at 5–30 rps with average query latency in the tens of milliseconds, the steady-state expected concurrent in-flight queries is well under 5. Even at p99 = 2s, peak concurrency would be in the 10–15 range. Idle connections recycle in 30 seconds. There is no mechanism by which steady traffic should accumulate connections from 2–3 to 35 over 14 hours.

That accumulation is only possible if connections are checked out and never returned. The implied leak rate (~32 connections / 14h ≈ 1 per ~25 min) at ~10 rps is consistent with **a rare-path leak** firing on roughly 1 in 15,000 requests — exactly the kind of corner case that aborted/cancelled requests would produce.

## Root Cause: Cancellation Path Bypasses pgClient Release

In `api/src/kg/postgraphile.ts:370-378`, the Yoga plugin chain is:

```ts
const sharedPlugins = [
    ...(responseCachePlugin ? [responseCachePlugin] : []),
    customValidationRules,
    useCostLogger(),
    useGraphQLInstrumentation(),
    useSearchInvocationLogger(),
    usePgClient(pgPool),         // checks out client in onExecute, releases in onExecuteDone
    useExecutionCancellation(),  // aborts execute() on AbortSignal
]
```

The `usePgClient` plugin (lines 226–323) follows the standard envelop pattern: `onExecute` checks out a client, `onExecuteDone` releases it. There is **no try/finally and no `onResponse`/`onRequestEnd`-style cleanup** that runs unconditionally.

When `useExecutionCancellation()` aborts the GraphQL `execute()` call (because the HTTP request's `AbortSignal` fired — e.g. a client closed the socket, producing a 499), two things go wrong:

1. **`onExecuteDone` is not guaranteed to fire on abort.** Yoga's runtime calls `onExecuteDone` after `execute()` resolves with a result. When execution is aborted, depending on the abort path, the executor may reject rather than resolve. The cleanup callback may simply never be invoked — the checked-out `pgClient` is then leaked indefinitely.

2. **node-pg's `PoolClient` has no `AbortSignal` integration.** Even when `onExecuteDone` does fire, the in-flight SQL on that client is still running. The pg driver does not propagate the GraphQL-level abort to a `pg_cancel_backend()` call. Calling `pgClient.release()` on a client with a pending query has undefined semantics: the client object may be returned to the pool while still bound to the query, and the underlying socket cannot serve another query until the original one finishes. In that window the connection is effectively unusable but still counted in `totalCount`.

Both failure modes leave the pool with a connection that nominally exists but cannot be reused until either:
- The query finishes naturally,
- Postgres `statement_timeout` (10s, per `api/docs/database-configuration.md:23`) kills it, or
- PgBouncer's `query_timeout` (15s) cuts the server side.

If any of those paths fail to fully clean the client object on the Node side (e.g. the `release()` was already called and the subsequent error doesn't re-enter the pool's bookkeeping), the connection persists in `totalCount` until pod restart.

The 499 line in the response codes chart growing throughout the day is the trigger condition. The leak rate (~1 every 25 min) is consistent with the fraction of requests where (a) the client cancels, (b) the GraphQL query had already begun executing, and (c) the release path hits one of the failure modes above.

## Secondary Issue: Timeout Hierarchy Is Incomplete

The user's hypothesis — that timeouts are misordered — is partially correct. The full chain in production today:

| Layer | Timeout | Source |
|---|---|---|
| nginx ingress | **not configured** → 60s default | `api/k8s/production/api.yaml:206-227` (no `proxy-read-timeout`/`proxy-send-timeout`) |
| Hono / Bun HTTP | **none** | not set anywhere |
| GraphQL execution | **none** | only `useExecutionCancellation`, which only fires on client abort |
| pg pool acquire | 3s | `PG_CONNECTION_TIMEOUT_MS` |
| pg `query_timeout` | **not set** | `api/src/kg/postgraphile.ts:50-62` |
| Postgres `statement_timeout` | 10s | `api/docs/database-configuration.md:23` |
| PgBouncer `query_timeout` | 15s | `api/docs/database-configuration.md:48` |
| pg idle | 30s | `PG_IDLE_TIMEOUT_MS` |

Issues:

- **No application-level request deadline.** A request can outlive the client's socket entirely — exactly the condition that triggers the leak path described above.
- **No `query_timeout` on the pg pool.** This setting destroys the connection client-side after N ms; without it, a "stuck" client (e.g. PgBouncer dropped the server side mid-query) sits in TCP read until either Postgres' `statement_timeout` kicks in or TCP times out. With `query_timeout`, the Node side reclaims the client deterministically.
- **nginx defaults to 60s read timeout**, which is fine in absolute terms but is a hidden default. It should be set explicitly so the timeout chain is auditable.

The current chain is not catastrophic — Postgres `statement_timeout=10s` is the one real safety net — but it's load-bearing in a way the surrounding configuration doesn't acknowledge.

## Secondary Issue: Saturation Threshold Trips on Brief Queues

`api/k8s/production/api.yaml:97-100`:

```yaml
- name: PG_POOL_PRESSURE_WAITING_THRESHOLD
  value: "1"
- name: PG_POOL_PRESSURE_UTILIZATION_THRESHOLD
  value: "90"
```

With `PRESSURE_WAITING_THRESHOLD=1`, the saturation FSM in `api/src/services/dbSaturation.ts` enters "pressured" state as soon as a single request waits for a pool slot, and flips to "saturated" (and starts shedding 503s — see `api/src/kg/postgraphile.ts:238-251`) after 15s of sustained pressure.

This is why 503s appear at "~50% utilization" rather than at the configured 90% — utilization itself isn't the trigger; even brief pool queues are. With a leaking pool, a queue forms long before utilization crosses 90%. The default of 5 (per `dbSaturation.ts:45`) is more tolerant of normal bursty traffic.

## Secondary Issue: Pool Metric Granularity

Today's pool gauges (`api/src/kg/postgraphile.ts:111-119`) emit `total_connections`, `idle_connections`, and `waiting_count` separately. The actually-diagnostic signal — **checked-out** (= total − idle) — has to be reconstructed downstream. A direct `checked_out` gauge would have made this leak visible immediately: under steady traffic it would climb, while `total` could be misread as "pool sizing up under load."

## Recommended Fix Set

In priority order. Each change is small and independently shippable.

### 1. Make `usePgClient` cleanup unconditional (highest impact)

**File:** `api/src/kg/postgraphile.ts`

The leak is in `usePgClient`'s reliance on `onExecuteDone`. Rework cleanup so it runs whether `execute()` resolved, threw, or was aborted. Two viable shapes:

**Option A — request-scoped cleanup via Yoga `onResponse`:**
- Track the checked-out client on `args.contextValue` (or a `WeakMap` keyed by the request).
- Move `pgClient.release()` to a Yoga `onResponse` (or `onRequestEnd`) hook, which fires regardless of whether execution completed normally.
- Keep the existing `onExecuteDone` error-aware release for the happy path; the response hook becomes a belt-and-braces guard.

**Option B — try/finally inside `onExecute`:**
- Wrap the result handling so `release()` runs in a `finally` block reachable from any path through the executor.
- Simpler to reason about but requires careful interaction with Yoga's plugin lifecycle.

Option A is preferred because it's idiomatic envelop/Yoga and handles all abort paths uniformly.

**Additionally:** when releasing a client whose query may still be in-flight, prefer `release(true)` (destroy the client) on cancellation paths — this severs the connection rather than returning a busy client to the pool. The cost is one new connection later; the benefit is no possibility of returning a wedged client.

### 2. Add `query_timeout` to the pool config

**File:** `api/src/kg/postgraphile.ts:50-62`

Add `query_timeout` to the `Pool` constructor, set just above the Postgres `statement_timeout` (e.g. 12_000 ms). This forces the Node-side pg client to abort the query and destroy the connection deterministically if the server-side timeout fails to free it. Treat it as the application-side fuse.

```ts
const pgPool = new Pool({
    // ... existing options
    query_timeout: parseInt(process.env.PG_QUERY_TIMEOUT_MS || "12000", 10),
})
```

Surface it as `PG_QUERY_TIMEOUT_MS` in `api/k8s/production/api.yaml` and `api/k8s/staging/api.yaml` so it's tunable per environment.

### 3. Set explicit nginx ingress timeouts

**File:** `api/k8s/production/api.yaml` (and staging counterpart)

Add to the ingress annotations:

```yaml
nginx.ingress.kubernetes.io/proxy-connect-timeout: "5"
nginx.ingress.kubernetes.io/proxy-send-timeout: "30"
nginx.ingress.kubernetes.io/proxy-read-timeout: "30"
```

Recommended order, longest to shortest: nginx (30s) > application deadline (20s, see #4) > Postgres `statement_timeout` (10s) > pool acquire (3s). This matches the hierarchy already documented in `api/docs/database-configuration.md:127-135`.

### 4. Add an application-level request deadline

**File:** `api/main.ts`

Add a Hono middleware on `/graphql` (and probably the REST routes) that creates an `AbortController`, races the response against a configurable timeout, and aborts the underlying handler if it expires. This guarantees that no request lives longer than the deadline, regardless of downstream behavior. Pair with a clear 504 response so the client distinguishes server-side timeout from network failure.

The deadline (e.g. 20s) sits *between* the nginx timeout and Postgres `statement_timeout` so the application owns its own kill switch.

### 5. Soften `PG_POOL_PRESSURE_WAITING_THRESHOLD`

**File:** `api/k8s/production/api.yaml:97-98`

Raise from `1` → `3` (or back to the default of `5`). Briefly waiting on the pool under bursty traffic is normal; treating a single waiting client as the start of a saturation episode produces premature 503s. Re-evaluate once #1 is shipped and the leak is gone — once the pool isn't accumulating phantom connections, true saturation should be rare and the threshold can be tuned on real signal.

### 6. Add a `checked_out` pool metric

**File:** `api/src/kg/postgraphile.ts:111-119`

Emit a fourth gauge:

```ts
Sentry.metrics.gauge("graphql.pool.checked_out_connections",
    stats.totalConnections - stats.idleConnections)
```

This single number is the leak detector. Under steady traffic it should oscillate at a stable level. Monotonic growth = leak. Add a Grafana panel on it next to the existing utilization chart.

## Verification Plan

After deploying #1 + #2:

1. Watch `graphql.pool.checked_out_connections` (after #6) over a 24-hour window. It should oscillate around the steady-state in-flight query count and reset to ~0 between bursts. Any monotonic growth means the fix is incomplete.
2. Confirm that 499 spikes no longer correlate with utilization growth. The original signature was 499s climbing, then utilization climbing, then 503s. After the fix, 499s should have no effect on utilization.
3. Confirm 503 rates fall to near-zero in normal operation. Any remaining 503s should now correspond to genuine saturation (real backpressure under real load), not false positives from the leak.

If utilization continues to climb after #1 + #2, the leak is somewhere else — re-run the analysis with the `checked_out` gauge as the primary signal and look for code paths that call `pool.connect()` outside `usePgClient` (none expected today; `api/src/services/storage/storage.ts` uses a separate REST pool).
