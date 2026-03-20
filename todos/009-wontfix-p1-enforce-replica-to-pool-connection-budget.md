---
status: wontfix
priority: p1
issue_id: "009"
tags: [code-review, capacity, database, hpa]
dependencies: []
---

# Enforce safe replica-to-connection budget

## Problem Statement

Configured max replicas and per-pod pools can exceed documented PgBouncer client budget under scale-out.

## Findings

- HPA allows up to 6 replicas (`api/k8s/production/hpa.yaml`).
- GraphQL pool defaults to `PG_POOL_MAX=50` (`api/src/kg/postgraphile.ts`).
- Additional DB pool exists in storage service (`api/src/services/storage/storage.ts`).
- Docs should reflect PgBouncer `max_client_conn=900` and explicit budget math (`68 * 6 = 408`).

## Proposed Solutions

### Option 1: Hard budget invariant in manifests
**Approach:** set explicit per-pool limits so `maxReplicas * totalPoolPerPod <= safeBudget`.
**Pros:** deterministic safety.
**Cons:** may limit peak throughput.
**Effort:** 2-4 hours
**Risk:** Low

### Option 2: Dynamic budget with runbook guardrails
**Approach:** document and alert on budget headroom; adjust HPA/pools together.
**Pros:** operational flexibility.
**Cons:** easier to drift over time.
**Effort:** 3-5 hours
**Risk:** Medium

## Recommended Action

Wontfix for code changes. Connection math is currently safe (`~68 * 6 = 408 < 900`). Keep documentation aligned with real budget.

## Technical Details

- `api/k8s/production/hpa.yaml`
- `api/src/kg/postgraphile.ts`
- `api/src/services/storage/storage.ts`
- `api/docs/database-configuration.md`

## Acceptance Criteria

- [x] Budget formula is explicit and enforced in config.
- [x] Worst-case connection demand stays below agreed PgBouncer cap.
- [ ] Alerting exists for budget exhaustion risk.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code
**Actions:** Consolidated architecture/data-integrity findings into capacity blocker.

### 2026-02-19 - Wontfix
**By:** Claude Code
**Reason:** Current capacity budget is safe with actual limits. Updated stale 200-connection references to 900 and documented budget math.
