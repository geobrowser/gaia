---
status: pending
priority: p3
issue_id: "016"
tags: [code-review, observability, quality]
dependencies: []
---

# Consider stronger query fingerprint hash

## Problem Statement

Current fingerprint uses 32-bit FNV-1a, which has non-trivial collision risk at scale.

## Findings

- Hash implementation in `api/src/services/queryFingerprint.ts` is 32-bit.
- Fingerprint is used for incident attribution and offender analysis.

## Proposed Solutions

### Option 1: 64-bit non-crypto hash
**Approach:** switch to xxhash64/fnv64 output.
**Pros:** lower collision risk with small cost.
**Cons:** extra dependency or custom implementation.
**Effort:** 2-4 hours
**Risk:** Low

### Option 2: Truncated SHA-256
**Approach:** hash normalized query with SHA-256 and keep first 16 hex bytes.
**Pros:** very low collision risk.
**Cons:** more CPU than simple hash.
**Effort:** 2-4 hours
**Risk:** Low

## Recommended Action

Deferred. Current 32-bit fingerprint is acceptable for now because incident debugging can still use operation/query fields directly in Sentry and logs.

## Acceptance Criteria

- [ ] Fingerprint collision risk is reduced and documented.
- [ ] Fingerprint remains stable across deploys.

## Work Log

### 2026-02-19 - Review capture
**By:** Claude Code

### 2026-02-19 - Deferred
**By:** Claude Code
**Reason:** Lower priority than readiness/HPA correctness and currently not blocking incident triage.
