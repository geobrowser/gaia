---
status: done
priority: p2
issue_id: "014"
tags: [code-review, testing, reliability]
dependencies: []
---

# Add tests for saturation hysteresis state machine

## Problem Statement

Critical readiness/saturation timing logic was added without direct tests.

## Findings

- New state machine in `api/src/services/dbSaturation.ts` has activation/release windows.
- No targeted tests were added in branch for boundary transitions.

## Proposed Solutions

### Option 1: Unit tests with fake clock
**Approach:** test activation, sustained pressure, release, and edge transitions by timestamp.
**Pros:** fast and deterministic.
**Cons:** less end-to-end coverage.
**Effort:** 3-5 hours
**Risk:** Low

### Option 2: Unit + integration probe behavior tests
**Approach:** add state machine tests plus health endpoint behavior tests.
**Pros:** stronger confidence.
**Cons:** more setup effort.
**Effort:** 5-8 hours
**Risk:** Low

## Recommended Action

Implemented deterministic unit tests for activation/release/pruning behavior.

## Acceptance Criteria

- [x] Activation/release hysteresis behavior is covered by tests.
- [x] Readiness status transitions are verified for key scenarios.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Added `dbSaturation.test.ts` and verified tests pass with `vitest run`.
