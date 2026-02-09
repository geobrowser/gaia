---
status: pending
priority: p3
issue_id: "027"
tags: [code-review, security, graphql]
dependencies: []
---

# `parseValue` error uses `String(value)` instead of `typeof value`

## Problem Statement

In `base58UuidPlugin.ts`, the `parseValue` error path uses `String(value)` which could leak content of non-string GraphQL variables into error messages. The `serialize` path in the same file correctly uses `typeof value`. This inconsistency is a minor security concern.

## Findings

- `api/src/kg/base58UuidPlugin.ts:16` — `parseValue` throws with `String(value)` in the error message
- `serialize` (line ~8) correctly uses `typeof value` — doesn't echo content
- GraphQL variables come from client input, so echoing them is a potential information leak
- Not high severity since GraphQL errors are typically returned to the caller anyway, but defense-in-depth

## Proposed Solutions

### Option 1: Use `typeof value` instead of `String(value)`

**Approach:** Change the error message from `String(value)` to `typeof value`, matching the pattern used in `serialize`.

**Pros:**
- Consistent with existing `serialize` pattern
- No content leakage
- Trivial fix

**Cons:**
- None

**Effort:** 5 minutes

**Risk:** None

## Acceptance Criteria

- [ ] `parseValue` error message uses `typeof value` not `String(value)`
- [ ] Consistent with `serialize` error pattern
- [ ] Existing tests pass

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Found inconsistency between `parseValue` and `serialize` error handling
- Confirmed `serialize` uses the safer `typeof value` pattern

**Learnings:**
- Error messages at I/O boundaries should never echo raw input — use type/length only
