---
status: done
priority: p2
issue_id: "013"
tags: [code-review, scaling, hpa, observability]
dependencies: []
---

# Add custom autoscaling signals for DB pressure

## Problem Statement

HPA still scales only on CPU/memory; DB/pool bottlenecks can degrade latency without triggering scale-out.

## Findings

- `api/k8s/production/hpa.yaml` uses resource metrics only.
- Alerts track p99/503 but do not feed autoscaling.
- Plan already calls out adapter/custom metric prerequisite.

## Proposed Solutions

### Option 1: Prometheus Adapter + external/custom metrics
**Approach:** expose pool-wait/acquire-timeout and ingress p99/503 metrics to HPA.
**Pros:** scaling aligns with actual bottleneck.
**Cons:** infra setup complexity.
**Effort:** 1-2 days
**Risk:** Medium

### Option 2: KEDA scaler path
**Approach:** use KEDA with Prometheus triggers for latency/error/saturation signals.
**Pros:** flexible trigger model.
**Cons:** introduces additional controller dependency.
**Effort:** 1-3 days
**Risk:** Medium

## Recommended Action

Implemented external metric autoscaling trigger with Prometheus Adapter.

## Acceptance Criteria

- [x] HPA can scale based on at least one DB-pressure-aligned metric.
- [x] Scale-up/scale-down behavior is validated under load.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Installed Prometheus Adapter (kubectl manifests), added external metrics APIService/config, and wired HPA external trigger `api_ingress_503_ratio_rate5m`.
