---
status: wontfix
priority: p3
issue_id: "017"
tags: [code-review, consistency, docs]
dependencies: []
---

# Align docs/status naming consistency

## Problem Statement

Minor naming inconsistencies reduce discoverability and pattern consistency.

## Findings

- Plan filename does not use the `-plan.md` suffix pattern seen in nearby dated plans.
- Readiness response uses `not_ready` while other statuses are single-token style.

## Proposed Solutions

### Option 1: Standardize now
**Approach:** rename plan file and switch readiness status token to `unready`.
**Pros:** cleaner conventions.
**Cons:** low-value churn.
**Effort:** <1 hour
**Risk:** Low

### Option 2: Leave as-is with style note
**Approach:** document acceptable variation, avoid churn.
**Pros:** no compatibility change.
**Cons:** conventions remain mixed.
**Effort:** <1 hour
**Risk:** Low

## Recommended Action

Wontfix. Cosmetic naming consistency is not worth churn for this incident-response PR.

## Acceptance Criteria

- [x] Naming convention decision is explicit and documented.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Wontfix
**By:** Claude Code
**Reason:** Low-value cleanup only; no reliability or operability impact.
