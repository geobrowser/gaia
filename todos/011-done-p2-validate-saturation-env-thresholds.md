---
status: done
priority: p2
issue_id: "011"
tags: [code-review, reliability, config]
dependencies: []
---

# Validate saturation env thresholds at startup

## Problem Statement

Threshold env values are parsed without validation; invalid values can silently disable saturation logic.

## Findings

- `parseInt` values in `api/src/services/dbSaturation.ts` are not bounded/validated.
- `NaN` comparisons can fail open and hide overload signals.

## Proposed Solutions

### Option 1: Central validated env parser
**Approach:** helper with defaults + min/max + `Number.isFinite` checks; throw on invalid.
**Pros:** robust and explicit.
**Cons:** small startup strictness change.
**Effort:** 1-3 hours
**Risk:** Low

### Option 2: Clamp invalid to defaults with warning
**Approach:** recover at runtime, emit warning logs.
**Pros:** avoids startup breakage.
**Cons:** can mask config mistakes.
**Effort:** 1-2 hours
**Risk:** Medium

## Recommended Action

Implemented strict startup validation with bounded integer ranges.

## Acceptance Criteria

- [x] Invalid env values cannot silently disable saturation logic.
- [x] Startup behavior is deterministic and documented.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Added validated env parsing in `dbSaturation` and documented fail-fast behavior in database config docs.
