---
status: complete
priority: p3
issue_id: "015"
tags: [code-review, cleanup, base58, pattern]
dependencies: []
---

# Add uuidToBase58(value: string) convenience function

## Problem Statement

The pattern `toBase58(toUuid(...))` appears 22 times across production code. A composed helper would reduce noise and make the intent clearer at call sites.

## Findings

- 22 occurrences of `toBase58(toUuid(...))` across production files
- Pattern always means "take a UUID in any format and produce Base58 output"
- A helper would be a one-liner: `export const uuidToBase58 = (v: string) => toBase58(toUuid(v))`
- Found by: code-simplicity-reviewer, pattern-recognition-specialist, architecture-strategist

## Proposed Solutions

### Option 1: Add uuidToBase58 convenience function

**Approach:** Add `export const uuidToBase58 = (value: string): string => toBase58(toUuid(value))` to `uuid.ts` and replace call sites.

**Pros:**
- Reduces 22 call sites to single function call
- Intent is clearer
- Less room for error in composition order

**Cons:**
- Another function in the API surface
- Arguably trivial composition

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Leave as-is

**Approach:** The two-function composition is explicit and clear. No change needed.

**Pros:**
- Each step is visible
- No new function to learn

**Cons:**
- Repetitive pattern across codebase

**Effort:** 0

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/uuid.ts` — new function
- 22 call sites across `profile/`, `proposals/`, `search/`, `versioned/`, `kg/`

## Acceptance Criteria

- [ ] Convenience function added (or decision documented to skip)
- [ ] Call sites updated if function added
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Counted 22 occurrences of the pattern
- Confirmed all follow identical composition order
