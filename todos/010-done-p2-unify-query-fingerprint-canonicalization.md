---
status: done
priority: p2
issue_id: "010"
tags: [code-review, observability, telemetry]
dependencies: []
---

# Unify query fingerprint source

## Problem Statement

Fingerprint calculation is inconsistent across paths, which can break correlation for long queries.

## Findings

- `api/src/kg/instrumentationPlugin.ts` fingerprints full printed query.
- `api/src/kg/postgraphile.ts` fingerprints truncated query (`slice(0, 2000)`).
- Same request can receive different fingerprints in spans vs timeout logs.

## Proposed Solutions

### Option 1: Canonical full normalized input
**Approach:** fingerprint full normalized query everywhere; truncate only display payload fields.
**Pros:** consistent correlation.
**Cons:** slightly more CPU for very long documents.
**Effort:** 1-2 hours
**Risk:** Low

### Option 2: Shared utility API with explicit modes
**Approach:** utility returns both canonical fingerprint and safe preview.
**Pros:** prevents regressions by design.
**Cons:** minor refactor.
**Effort:** 2-3 hours
**Risk:** Low

## Recommended Action

Implemented canonical fingerprinting from full query input.

## Acceptance Criteria

- [x] One canonical fingerprint for same logical query across all telemetry paths.
- [x] Logs still respect payload size limits.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Updated PostGraphile path to hash full query while continuing to truncate query text only for log payload size limits.
