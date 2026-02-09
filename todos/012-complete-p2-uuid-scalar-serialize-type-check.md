---
status: complete
priority: p2
issue_id: "012"
tags: [code-review, type-safety, graphql, tigerstyle]
dependencies: []
---

# uuidScalarPlugin.serialize calls String(value) on unknown without type check

## Problem Statement

In `uuidScalarPlugin.ts`, the `serialize` path calls `String(value)` on unknown without checking type first. If `value` is `null`/`undefined`/object, `String(value)` produces `"null"`/`"undefined"`/`"[object Object]"` which `toUuid` rejects with an unhelpful error. The `parseValue` path correctly checks `typeof value !== "string"` first — serialize should too.

## Findings

- `serialize` in `uuidScalarPlugin.ts` does `String(value)` then passes to `toUuid` → `toBase58`
- `parseValue` correctly checks `typeof value !== "string"` before processing
- Asymmetry between the two paths — serialize is less defensive
- Found by: tigerstyle-reviewer

## Proposed Solutions

### Option 1: Add typeof guard to serialize

**Approach:** Add `if (typeof value !== "string") throw new GraphQLError(...)` before `String(value)` in serialize.

**Pros:**
- Consistent with parseValue pattern
- Clear error message for non-string values
- Simple fix

**Cons:**
- None

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/kg/uuidScalarPlugin.ts` — `serialize` function

## Acceptance Criteria

- [ ] `serialize` checks `typeof value` before calling `String(value)`
- [ ] Non-string values produce clear GraphQL error
- [ ] Test added for null/undefined/object serialize inputs
- [ ] Existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified asymmetry between serialize and parseValue input validation
