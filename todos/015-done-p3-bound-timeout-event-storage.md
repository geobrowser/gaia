---
status: done
priority: p3
issue_id: "015"
tags: [code-review, performance, reliability]
dependencies: []
---

# Bound timeout event storage in saturation tracker

## Problem Statement

Timeout tracking currently uses append+filter arrays, which can add avoidable overhead during incident bursts.

## Findings

- `api/src/services/dbSaturation.ts` stores timestamps in arrays and prunes via filter.
- Under high failure rates this can increase GC/CPU in a hot path.

## Proposed Solutions

### Option 1: Ring buffer cap
**Approach:** keep fixed-size queue per pool, evict oldest on insert.
**Pros:** bounded memory and predictable cost.
**Cons:** slight implementation complexity.
**Effort:** 2-4 hours
**Risk:** Low

### Option 2: Time bucket counters
**Approach:** aggregate counts per second/minute buckets over rolling window.
**Pros:** O(1) updates, low memory.
**Cons:** less granular timestamps.
**Effort:** 3-6 hours
**Risk:** Low

## Recommended Action

Implemented time-bucket tracking with bounded memory and O(1)-ish updates.

## Acceptance Criteria

- [x] Event tracking has explicit bounded memory.
- [x] Saturation logic behavior remains equivalent for configured window.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Replaced timestamp arrays with per-second bucket counters in `dbSaturation` and re-ran related tests.
