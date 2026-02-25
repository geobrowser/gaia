---
status: done
priority: p2
issue_id: "012"
tags: [code-review, security, observability]
dependencies: []
---

# Reduce public readiness telemetry detail

## Problem Statement

Readiness currently exposes detailed pool pressure internals in a publicly reachable API path.

## Findings

- `/health/readiness` returns detailed `poolPressure` payload in `api/src/health.ts`.
- Endpoint is available through normal ingress routes.
- This can reveal internal saturation state to external actors.

## Proposed Solutions

### Option 1: Minimal readiness payload
**Approach:** return only status/reason, move details to internal-only endpoint.
**Pros:** reduces information leakage.
**Cons:** less immediate detail from public probe path.
**Effort:** 1-2 hours
**Risk:** Low

### Option 2: Keep detail but restrict network access
**Approach:** enforce ingress/network policy restrictions for health endpoints.
**Pros:** preserves operator detail.
**Cons:** infra dependency and policy complexity.
**Effort:** 2-5 hours
**Risk:** Medium

## Recommended Action

Implemented minimal readiness payload for public path.

## Acceptance Criteria

- [x] External callers cannot access detailed saturation internals.
- [x] Kubernetes probe behavior remains unchanged.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Completed
**By:** Claude Code
**Actions:** Reduced `/health/readiness` response to status/reason/timestamp and removed detailed pool internals from that endpoint.
