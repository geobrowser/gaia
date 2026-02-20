---
title: fix: PostGraphile pool timeout and DB tail-latency investigation
type: fix
date: 2026-02-19
---

# fix: PostGraphile pool timeout and DB tail-latency investigation

## Incident Summary

- During 2026-02-19 20:55-21:03 UTC, `gaia-api` saw a burst of `PostGraphile pool connect error` / `timeout exceeded when trying to connect`.
- Failures were concentrated on one API pod (`api-76bf4ffb55-5mt6m`), while the sibling pod did not show the same error burst.
- Request failures clustered at about 10 seconds, matching both API connect timeout (`PG_CONNECTION_TIMEOUT_MS=10000`) and DB `statement_timeout=10s`.

## What We Verified

- Kubernetes resource pressure was not obvious (CPU, memory, packet drops, conntrack all low/normal for involved pods/nodes).
- Postgres was not near `max_connections` at check time.
- `statement_timeout=10s`, `lock_timeout=0`, `idle_in_transaction_session_timeout=30s`, `log_lock_waits=on`, `log_min_duration_statement=1s`.
- Hermes slow `process_block` traces in the same window were dominated by `prefetch`, not edit transform/emit/db commit.
- `ipfs_cache` rows for the suspect edit blocks (`96070`, `96071`, `96073`) were tiny (`803B`, `220B`, `181B`).

## Working Hypothesis

- This pattern is consistent with transient tail-latency spikes and pool starvation rather than a single always-slow query.
- One pod can fail while another remains healthy when saturation is process-local or path-local (pool queue, event-loop stall, connection churn, or route imbalance).
- Intermittent DB wait conditions (lock wait, checkpoint/storage jitter, or pooled connection contention) can produce p99 outliers while p50 stays fast.

## Decisions

1. Enable better offender visibility:
   - Add `pg_stat_statements` and retain enough history for incident windows.
   - Add query fingerprint / operation mapping in app telemetry.
2. Separate timeout budgets:
   - Keep connect/acquire timeout shorter than statement timeout.
   - Avoid equal 10s caps for both pool connect and statement execution.
3. Add lock-wait control:
   - Set non-zero `lock_timeout` below `statement_timeout` so lock contention fails fast and is classifiable.
4. Add per-pod pool instrumentation:
   - Emit `totalCount`, `idleCount`, `waitingCount`, checkout wait duration, and acquire timeout count by pod.
5. Add overload routing/protection:
   - Use readiness that fails on sustained pool saturation (not transient blips).
   - Add admission control for DB-heavy GraphQL operations to avoid queue blowups.

## Open Why-Questions

- Which exact normalized SQL fingerprints account for tail spikes during incident windows?
- Were p99 outliers lock-driven, I/O/checkpoint-driven, or pooler-queue-driven?
- Why did one pod exhibit sustained timeout behavior while sibling pod did not (traffic skew, local runtime stall, or connection-path issue)?

## Next Validation Steps

1. Capture lock/blocker chains during events (`log_lock_waits`, blocker PID correlation).
2. Compare per-pod request concurrency and pool queue depth in the same minute bucket as failures.
3. Add alerting on sustained pool `waitingCount > 0` by pod, not only global error rate.
4. Build a weekly p99 offender report from `pg_stat_statements` + GraphQL operation names.

## Implementation Checklist

### A) Route Traffic Away From Saturated Pods

1. Add a lightweight in-process saturation signal in API:
   - Compute `isSaturated` from pool metrics over a short rolling window (for example 15-30s):
     - `waitingCount > 0` for sustained duration, or
     - acquire/connect timeout count above threshold.
2. Wire saturation into readiness (not liveness):
   - readiness should fail only on sustained saturation (hysteresis), not one spike.
   - liveness remains event-loop/process health only to avoid restart cascades.
3. Keep readiness response structured:
   - include pool snapshot (`totalCount`, `idleCount`, `waitingCount`, recent acquire timeout count) for operators.
4. Defer route-level overload shed until readiness and observability baselines are in place.
5. Add alerts for pod-level imbalance:
   - alert on one pod showing sustained `waitingCount > 0` while siblings remain near zero.

Acceptance checks:
- Saturated pod drops out of Endpoints quickly.
- Sibling pods continue serving without matching 10s timeout bursts.
- No liveness-driven restart loops during transient pressure.

### B) Capture the "Why" for p99 Spikes

1. Enable `pg_stat_statements` in Postgres.
2. Add app-level query fingerprint mapping:
   - log normalized query hash/fingerprint + GraphQL operation name + request id.
3. Add explicit pool timing metrics:
   - checkout/acquire wait duration histogram,
   - acquire timeout counter,
   - active/idle/waiting counts by pod.
4. Add lock/wait classification from DB logs:
   - parse lock wait and statement timeout entries,
   - correlate blocker PID -> blocked PID and mapped API request ids.
5. Add a recurring offender report:
   - top SQL fingerprints by total time and p95/p99,
   - top fingerprints by timeout count,
   - top GraphQL operations mapped to those fingerprints.

Acceptance checks:
- For any timeout spike, on-call can answer within 10 minutes:
  - which fingerprint,
  - whether lock/I/O/pooler queue,
  - which pod(s),
  - and which API operation names were impacted.

### C) Timeout Hierarchy and Guardrails

1. Set explicit timeout ordering:
   - pool acquire/connect timeout < request deadline < statement timeout.
2. Set non-zero `lock_timeout` below `statement_timeout` to make lock contention obvious.
3. Keep `statement_timeout` high enough to avoid false positives but low enough to protect capacity.
4. Roll out timeout changes with canary + before/after dashboards.

Acceptance checks:
- Fewer ambiguous 10s failures.
- Error classes separate cleanly (`pool_connect_timeout` vs `statement_timeout` vs lock timeout class).

## Suggested Execution Order (1 Sprint)

1. Observability first: `pg_stat_statements` + pool wait metrics + fingerprint mapping.
2. Add readiness saturation hysteresis and pod-level alerts.
3. Tune timeout hierarchy and validate with replay/load tests.

## HPA Capacity Plan

### Immediate (implemented)

- Keep `maxReplicas` bounded by PgBouncer connection budget.
- Add explicit HPA behavior so scale-up reacts quickly and scale-down is conservative.
- Alert when HPA is maxed while p99 remains elevated.

### Next Step (custom metric autoscaling)

Add autoscaling signals that track overload directly (not only CPU/memory):

1. `api:ingress_p99_latency_seconds:5m`
2. `api:ingress_503_ratio:rate5m`
3. Pod-level pool wait/acquire timeout metric (export from API process)

Target policy:

- scale out when p99 and/or 503 ratio stays high for 5-10m,
- scale down only after sustained recovery,
- keep max replicas fixed to connection-budget-safe value.

Prerequisite: expose app pool-pressure counters as Prometheus metrics and wire Prometheus Adapter (or equivalent) for HPA custom/external metrics.

## Implementation Status (2026-02-19)

Completed:

- Readiness-based saturation drain is implemented (`/health/readiness`) and wired in staging + production manifests.
- Pool pressure hysteresis and acquire-timeout tracking are implemented in API runtime.
- GraphQL timeout events now include pool pressure context and query fingerprint (`graphql.query_fingerprint`).
- Production HPA behavior now has explicit fast scale-up and conservative scale-down policy.
- Capacity alerts added for readiness degradation, HPA max + high p99, and elevated 503 ratio.
- Database updates applied:
  - `CREATE EXTENSION IF NOT EXISTS pg_stat_statements;`
  - `ALTER DATABASE defaultdb SET lock_timeout = '2s';`

Remaining:

- Add Prometheus Adapter (or equivalent) and wire custom/external metrics to HPA.
- Build the weekly offender report automation from `pg_stat_statements` + operation mappings.
