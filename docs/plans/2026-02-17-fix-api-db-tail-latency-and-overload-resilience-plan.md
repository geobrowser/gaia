---
title: fix: Reduce API DB tail latency and improve overload resilience
type: fix
date: 2026-02-17
---

# fix: Reduce API DB tail latency and improve overload resilience

## Overview

The API shows two recurring production failure classes that both surface as client-facing failures: pool checkout timeouts (`timeout exceeded when trying to connect`) and request abort/connection-closed failures (`AbortError: The connection was closed`). Median latency is healthy, but tail latency and failure-mode ambiguity cause intermittent instability and make incident triage slow.

This plan defines a phased approach to:
- identify slow SQL offenders quickly and continuously,
- separate failure classes in telemetry and alerts,
- degrade gracefully under stress,
- and reduce p99 latency while preserving correctness on write paths.

## Problem Statement

### What is happening

- API has two separate PostgreSQL pools:
  - GraphQL/PostGraphile pool (`api/src/kg/postgraphile.ts:31`)
  - REST/Drizzle pool (`api/src/services/storage/storage.ts:24`)
- Both pools can fail independently, but incidents are currently perceived as one generic timeout problem.
- Production evidence shows at least two different failure classes in recent windows:
  - pool checkout timeout class (`timeout exceeded when trying to connect`)
  - connection-abort class (`AbortError: The connection was closed`)

### Why this is risky

- Tail behavior (`p99`) drives concurrency pressure more than median (`p50`).
- Error classification is not explicit enough for fast operator decisions.
- Ingress status visibility and app-level status/trace visibility can diverge (for example, client-abort style outcomes).
- Without explicit offender ranking, teams optimize reactively instead of continuously.

## Research Summary

Found brainstorm from 2026-02-03: `trace-observability-sentry-sampling` (`docs/brainstorms/2026-02-03-trace-observability-brainstorm.md`). Using as context for planning.

### Local repository findings

- GraphQL pool includes explicit connect timeout and pool error handlers (`api/src/kg/postgraphile.ts:38`, `api/src/kg/postgraphile.ts:57`).
- REST pool includes explicit connect timeout and pool stats helper (`api/src/services/storage/storage.ts:34`, `api/src/services/storage/storage.ts:69`).
- Pool health endpoints exist for both paths (`api/src/health.ts:223`, `api/src/health.ts:276`).
- Request start/end logging and request IDs are standardized (`api/src/middleware/requestLogging.ts:57`, `api/src/middleware/requestLogging.ts:95`).
- Current ingress observability emphasizes total/5xx/latency; status-code expansion was added and should be used for incident triage (`monitoring/k8s/api-ingress-rules.yaml:26`, `monitoring/k8s/api-ingress-dashboard.yaml:250`).
- Database-level timeouts are documented as `statement_timeout=10s` and PgBouncer `query_timeout=15s` (`api/docs/database-configuration.md:23`, `api/docs/database-configuration.md:48`).

### Institutional learnings

- Treat liveness as event-loop health only; keep DB checks out of liveness to avoid restart cascades (`api/src/health.ts:8`, `api/src/health.ts:33`).
- Destroy suspicious pg clients on GraphQL error path to avoid stale connection reuse (`api/src/kg/postgraphile.ts:163`).
- Use strict request-id propagation and structured start/end logs as canonical incident breadcrumbs (`api/src/middleware/requestLogging.ts:19`, `api/src/middleware/requestLogging.ts:33`).
- Prefer measured reliability fixes with paired observability updates (recent fixes in this area: `01992b0`, `90e2fa8`, `be0d818`).

### External research decision

Skipped external research for this plan iteration. Rationale: this codebase already has strong local patterns, incident artifacts, and in-repo operational guidance specific to this stack.

## Proposed Solution

Implement a reliability program with two parallel tracks:

1. **Catch offenders:** Build an always-on SQL offender pipeline that maps slow DB work to API operation names and request IDs.
2. **Graceful stress behavior:** Normalize overload semantics and apply admission control so bursts fail predictably instead of cascading.

## Technical Approach

### Architecture

#### A) Observability and offender detection

- Introduce a stable offender scorecard sourced from:
  - `pg_stat_statements` (frequency, total time, tail indicators)
  - API spans/logs (request id, operation name, route)
  - pool pressure indicators (`waitingCount`, utilization)
- Produce one operator-first view that answers:
  - which query fingerprints consume the most DB time,
  - which API operations map to those fingerprints,
  - whether failures are pool timeouts vs connection aborts.

#### B) Stress handling and user-facing behavior

- Define explicit failure taxonomy and response mapping:
  - `pool_connect_timeout`
  - `connection_closed_abort`
  - `statement_timeout`
  - `unknown_db_failure`
- Add route-level admission control for DB-heavy paths to prevent queue blowups.
- Keep timeout hierarchy intentional: checkout timeout < app deadline < statement timeout.

#### C) Correctness guarantees

- For write endpoints, define unknown-outcome policy for aborted connections:
  - idempotency keys where applicable,
  - safe retry policy,
  - reconciliation steps for operators.

### Implementation Phases

#### Phase 1: Instrumentation and Classification Foundation

Goal: classify failures unambiguously and capture offender evidence without behavior changes.

Tasks:
- [x] Add normalized error classification fields in logs and Sentry contexts.
- [x] Ensure every DB-related error path includes pool stats snapshot where available.
- [x] Add dashboards and queries that separate timeout vs abort classes.

Execution notes (2026-02-17):
- Implemented `DbFailureClass` taxonomy and detection helpers in `api/src/services/dbFailures.ts`.
- Added failure class + pool stats context to GraphQL pool errors in `api/src/kg/postgraphile.ts`.
- Added graceful overload response mapping for GraphQL pool checkout timeout (`503` + `Retry-After`) in `api/main.ts`.
- Added middleware error-class span/log enrichment in `api/src/middleware/requestLogging.ts`.
- Added ingress per-status visibility panels/rules in `monitoring/k8s/api-ingress-dashboard.yaml` and `monitoring/k8s/api-ingress-rules.yaml`.

Implementation files:
- `api/src/kg/postgraphile.ts`
- `api/src/services/storage/storage.ts`
- `api/src/services/telemetry.ts`
- `monitoring/k8s/api-ingress-dashboard.yaml`
- `monitoring/k8s/api-ingress-rules.yaml`

Success criteria:
- 95%+ of DB/path failures are auto-classified into one of the defined classes.
- Operators can identify top 5 offender operations from one dashboard within 10 minutes.

Estimated effort: 1-2 days.

#### Phase 2: Offender Triage and Tail-Reduction Loop

Goal: reduce p99 by systematically addressing top SQL offenders.

Tasks:
- Create weekly top-offender list from `pg_stat_statements` + API operation mapping.
- For top 3 offenders per cycle, run execution-plan analysis and apply targeted query/index fixes.
- Enforce pagination and query-shape limits on high-cost endpoints.

Execution notes (2026-02-17):
- Added a concrete Phase 2 SQL offender query pack and weekly triage template in `api/docs/database-configuration.md`.
- Added ingress dashboard/rules panels for failure-class proxy rates and top routes by 5xx/503 to speed initial offender localization.

Implementation files:
- `api/docs/database-configuration.md` (operational query pack updates)
- `api/src/proposals/router.ts` (high-traffic DB-heavy paths)
- `api/src/versioned/router.ts` (degraded-mode examples and guardrails)
- DB migration files as needed (if index changes are required)

Success criteria:
- First milestone: p99 reduced from current baseline toward < 800ms.
- Offender backlog maintained and re-ranked weekly.
- No correctness regression in write flows.

Estimated effort: ongoing, with first measurable improvement in 1 sprint.

#### Phase 3: Graceful Degradation Under Stress

Goal: prevent cascading failures during bursts and tail spikes.

Tasks:
- Map pool checkout timeout to explicit overload response contract.
- Add per-route admission control caps for DB-heavy operations.
- Define and implement degraded-mode entry/exit criteria with hysteresis.

Implementation files:
- `api/main.ts`
- `api/src/kg/postgraphile.ts`
- `api/src/health.ts`
- `monitoring/k8s/api-ingress-rules.yaml`

Success criteria:
- Under synthetic stress, system degrades predictably without multi-minute collapse.
- Recovery to baseline occurs automatically after pressure drops.
- Alerting separates overload from abort/cancellation spikes.

Estimated effort: 2-4 days including verification drills.

## Alternative Approaches Considered

- **Only increase pool/connect timeouts:** rejected as primary strategy. It can mask saturation and increase queueing delay.
- **Only scale pool sizes upward:** rejected as standalone fix. Without tail control and classification, this delays symptoms but does not remove root causes.
- **Only tune SQL without overload controls:** rejected. Tail improvements help, but stress behavior still needs explicit guardrails.

## Acceptance Criteria

### Functional Requirements

- [ ] Failures are classified into explicit categories with stable names in logs/traces.
- [ ] A repeatable weekly SQL offender report exists and is used.
- [ ] On-call runbook includes triage decision tree by failure class.
- [ ] API behavior under overload is deterministic and documented.

### Non-Functional Requirements

- [ ] p99 API latency trend improves materially from baseline.
- [ ] Pool waiting events are visible and alertable for both GraphQL and REST pools.
- [ ] Overload response contract avoids ambiguous generic 500 behavior.
- [ ] Cancellation/abort outcomes are measurable and distinguishable from DB saturation.

### Quality Gates

- [ ] Regression tests for error mapping and overload semantics.
- [ ] Load-test scenarios for timeout-only, abort-only, and mixed-mode failures.
- [ ] Dashboards and alerts validated by replaying known historical windows.

## Success Metrics

- p95 and p99 latency (per route class and globally).
- Pool `waitingCount` duty cycle and peak duration.
- Rate of `pool_connect_timeout` vs `connection_closed_abort` vs `statement_timeout`.
- Time-to-identify top offender during incident (target: < 10 minutes).
- Client-visible error-rate reduction during burst windows.

## Dependencies and Prerequisites

- Access to production metrics and Sentry event APIs.
- `pg_stat_statements` available and retained at useful horizon.
- Agreement on API overload response semantics.
- Alignment with platform owners on ingress/app timeout hierarchy.

## Risk Analysis and Mitigation

- **Risk:** Over-classification complexity adds noise.
  - **Mitigation:** Keep taxonomy small and stable, start with four classes.
- **Risk:** Tail fixes target low-impact queries.
  - **Mitigation:** Rank by total DB time share, not only max latency.
- **Risk:** Admission control too strict hurts good traffic.
  - **Mitigation:** progressive rollout with per-route tuning and rollback thresholds.
- **Risk:** Write correctness under aborts is ambiguous.
  - **Mitigation:** explicit idempotency/reconciliation contract before rollout.

## Resource Requirements

- Backend engineer(s): API instrumentation and routing semantics.
- Data/DB engineer support: offender triage and plan analysis.
- SRE/Platform support: dashboards, alert thresholds, stress drills.

## Future Considerations

- Add endpoint-specific query budgets and adaptive concurrency limits.
- Consider query-shape budgeting in GraphQL for expensive resolver combinations.
- Consider automated offender issue creation from weekly ranking output.

## Documentation Plan

- Update runbook with failure taxonomy and triage flow.
- Extend database configuration docs with offender query pack and operational thresholds.
- Document overload response contract and retry guidance for clients.

## SpecFlow Gap Coverage

The plan includes SpecFlow-identified gaps:

- separate flows for timeout vs abort classes,
- explicit unknown-outcome handling for write paths,
- degraded-mode entry/exit hysteresis,
- phased rollout with mixed-failure verification.

## References and Research

### Internal References

- Pool config and connect timeout:
  - `api/src/kg/postgraphile.ts:31`
  - `api/src/kg/postgraphile.ts:38`
  - `api/src/services/storage/storage.ts:24`
  - `api/src/services/storage/storage.ts:34`
- Pool health endpoints:
  - `api/src/health.ts:223`
  - `api/src/health.ts:276`
- Request logging and request ID propagation:
  - `api/src/middleware/requestLogging.ts:19`
  - `api/src/middleware/requestLogging.ts:57`
  - `api/src/middleware/requestLogging.ts:95`
- Telemetry wiring:
  - `api/src/services/telemetry.ts:31`
  - `api/src/services/telemetry.ts:66`
- Ingress observability:
  - `monitoring/k8s/api-ingress-rules.yaml:12`
  - `monitoring/k8s/api-ingress-rules.yaml:26`
  - `monitoring/k8s/api-ingress-dashboard.yaml:13`
  - `monitoring/k8s/api-ingress-dashboard.yaml:250`
- DB timeout docs:
  - `api/docs/database-configuration.md:23`
  - `api/docs/database-configuration.md:48`
- Relevant brainstorm context:
  - `docs/brainstorms/2026-02-03-trace-observability-brainstorm.md`

### Related Work

- `01992b0` fix(api): capture GraphQL context on pool checkout timeout
- `90e2fa8` fix: increase API pg connect timeout and add ingress visibility
- `be0d818` fix(api): add error handling to PostGraphile connection pool
