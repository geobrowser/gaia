---
status: done
priority: p2
issue_id: "004"
tags: [code-review, correctness, proposals]
dependencies: []
---

# Assert SELECT EXISTS contract instead of silent fallback

## Problem Statement

The `hasActiveProposalForTarget` function falls back to `false` when the query returns no rows (`result.rows[0]?.exists ?? false`). However, `SELECT EXISTS(...)` in PostgreSQL **always** returns exactly one row. Zero rows would indicate a fundamental query execution failure that should be surfaced, not silently swallowed as `false`.

The test suite (queries.test.ts:680-688) validates this incorrect fallback behavior rather than asserting the contract.

## Findings

- `queries.ts:503` — `result.rows[0]?.exists ?? false` silently defaults
- PostgreSQL `SELECT EXISTS(...)` always returns exactly 1 row with a boolean value
- Zero rows would indicate a broken connection, query rewrite, or middleware interference
- The test "returns { active: false } when query returns no rows" encodes this behavior as expected
- TigerStyle review flagged this as a contract gap

## Proposed Solutions

### Option 1: Assert row count, remove fallback

**Approach:** Assert that exactly 1 row is returned, then access it directly. Remove the "empty rows" test case or change it to expect an error.

```typescript
const rows = result.rows
if (rows.length !== 1) {
  throw new Error(`SELECT EXISTS must return exactly 1 row, got ${rows.length}`)
}
return rows[0]!.exists
```

**Pros:**
- Fails fast on broken invariant instead of returning wrong answer
- Catches infrastructure issues early

**Cons:**
- Slightly less defensive (but the defensiveness was hiding bugs)

**Effort:** 15 minutes

**Risk:** Low

---

### Option 2: Keep fallback but log a warning

**Approach:** Keep the `?? false` fallback but add structured logging when it triggers.

**Pros:**
- Doesn't change behavior for callers
- Still surfaces the issue via logs

**Cons:**
- Silently returns wrong answer to the caller

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- `api/src/proposals/queries.ts:503` — the fallback line
- `api/src/proposals/__tests__/queries.test.ts:680-688, 762-770` — the "empty rows" test cases

## Resources

- **PR:** #400
- **Review agents:** tigerstyle-reviewer

## Acceptance Criteria

- [x] Zero-row result from SELECT EXISTS is treated as an error, not a silent false
- [x] Tests updated to reflect the correct contract
- [x] All other tests pass

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Verified PostgreSQL SELECT EXISTS always returns 1 row
- Identified the test that encodes the wrong behavior
- Assessed impact: false negative could cause duplicate proposal creation

**Learnings:**
- Defensive fallbacks can hide real failures
- SELECT EXISTS is one of the few SQL patterns with a guaranteed row count

### 2026-02-13 - Completed (Option 1)

**By:** Claude Code

**Actions:**
- Changed `result.rows[0]?.exists ?? false` to throw if zero rows returned
- Improved error wrapping: `error instanceof Error ? error : new Error(String(error))`
- Updated test cases to expect 500 on zero rows instead of silent `{ active: false }`
- All 52 tests pass, TypeScript clean
