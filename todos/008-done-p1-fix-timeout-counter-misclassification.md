---
status: done
priority: p1
issue_id: "008"
tags: [code-review, reliability, observability]
dependencies: []
---

# Count only real acquire timeouts

## Problem Statement

The acquire-timeout counter is incremented for every `pool.connect()` failure, including non-timeout errors.

## Findings

- `recordGraphqlAcquireTimeout()` runs unconditionally in `api/src/kg/postgraphile.ts` catch path.
- Saturation logic consumes this signal in `api/src/services/dbSaturation.ts`.
- Non-timeout failures can falsely mark pod as saturated and trigger readiness drain.

## Proposed Solutions

### Option 1: Class-gated increment
**Approach:** call timeout recorder only when `classifyDbFailure(err) === "pool_connect_timeout"`.
**Pros:** precise signal, minimal change.
**Cons:** depends on classifier quality.
**Effort:** 1-2 hours
**Risk:** Low

### Option 2: Split counters by failure class
**Approach:** maintain separate counters for timeout vs other connect failures.
**Pros:** richer diagnostics.
**Cons:** slightly more code/metrics surface.
**Effort:** 2-4 hours
**Risk:** Low

## Recommended Action

Implemented class-gated timeout recording.

## Technical Details

- `api/src/kg/postgraphile.ts`
- `api/src/services/dbSaturation.ts`

## Acceptance Criteria

- [x] Non-timeout connect errors do not increment timeout signal.
- [x] Saturation decisions reflect only intended timeout pressure.
- [x] Existing timeout alerts still trigger correctly.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code
**Actions:** Marked as critical due to false-positive readiness impact.

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Updated GraphQL pool connect error path to increment timeout counter only for `pool_connect_timeout` failures.
