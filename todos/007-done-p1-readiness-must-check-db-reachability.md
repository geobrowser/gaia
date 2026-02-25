---
status: done
priority: p1
issue_id: "007"
tags: [code-review, reliability, k8s, database]
dependencies: []
---

# Add DB reachability to readiness

## Problem Statement

`/health/readiness` currently gates only on saturation heuristics, not direct DB availability.

## Findings

- `api/src/health.ts` readiness uses `getGraphqlPoolPressure()` only.
- Probes now target `/health/readiness` in `api/k8s/production/api.yaml` and `api/k8s/staging/api.yaml`.
- If DB is down while saturation is false, pods can remain Ready and continue receiving failing traffic.

## Proposed Solutions

### Option 1: Inline fast DB check in readiness
**Approach:** run bounded `SELECT 1` in readiness and fail on timeout/error.
**Pros:** direct correctness, easy to reason about.
**Cons:** adds DB probe load.
**Effort:** 2-4 hours
**Risk:** Medium

### Option 2: Background cached DB health signal
**Approach:** periodic DB probe updates shared state; readiness reads cached result + saturation.
**Pros:** lower probe overhead, stable behavior.
**Cons:** more moving parts.
**Effort:** 4-8 hours
**Risk:** Medium

## Recommended Action

Implemented with bounded DB probe in readiness.

## Technical Details

- `api/src/health.ts`
- `api/k8s/production/api.yaml`
- `api/k8s/staging/api.yaml`

## Acceptance Criteria

- [x] Readiness fails when DB is unreachable.
- [x] Readiness still drains saturated pods.
- [x] No probe-induced restart loop.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code
**Actions:** Consolidated multi-agent findings; marked as merge-blocking reliability risk.

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Added readiness DB reachability check with bounded timeout and kept saturation gating.
