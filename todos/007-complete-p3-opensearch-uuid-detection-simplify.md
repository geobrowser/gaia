---
status: complete
priority: p3
issue_id: "007"
tags: [code-review, simplicity, cleanup]
dependencies: []
---

# Simplify OpenSearch UUID detection from two branches to one

## Problem Statement

In `opensearch.ts`, UUID query detection was split into two sequential if-blocks: one for dashed hex (fast path) and one for any UUID format via `isValidUuid()`. Since `toUuid()` on a dashed hex string is just a regex test + toLowerCase() (negligible cost), the fast path adds cognitive load for no meaningful performance gain in a search endpoint.

## Findings

- `opensearch.ts:197-202`: Two sequential UUID detection blocks
- `toUuid()` on already-dashed input costs ~0.1us (regex match + toLowerCase)
- Search endpoints are not a hot path (user-initiated, not bulk)
- Simplification saves ~3 LOC

## Proposed Solutions

### Option 1: Collapse to single isValidUuid check

**Approach:** Replace both if-blocks with:
```ts
if (isValidUuid(trimmedQuery)) {
    return this.buildUuidQuery(toUuid(trimmedQuery), ...)
}
```

**Effort:** 5 minutes

**Risk:** None

## Recommended Action

*To be filled during triage.*

## Acceptance Criteria

- [ ] Single UUID detection branch in opensearch.ts
- [ ] All tests pass

## Work Log

### 2026-02-09 - Simplicity Review Discovery

**By:** Claude Code (code-simplicity-reviewer)
