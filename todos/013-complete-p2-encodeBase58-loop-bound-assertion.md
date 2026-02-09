---
status: complete
priority: p2
issue_id: "013"
tags: [code-review, safety, base58, tigerstyle]
dependencies: []
---

# encodeBase58 loop should assert iteration bound

## Problem Statement

The `while (remainder > 0n)` loop in `encodeBase58` is provably bounded at 22 iterations for 128-bit input, but TigerStyle demands the bound be asserted at runtime, not just reasoned about. A bug in surrounding code could theoretically feed an oversized value.

## Findings

- `encodeBase58` loop divides by 58 until remainder is 0
- For 128-bit input (max value 2^128-1), this is mathematically bounded at ceil(128 / log2(58)) ≈ 22
- The precondition checks input is 32 hex chars, which bounds input to 128 bits
- But TigerStyle principle: "assert the bound, don't just reason about it"
- Found by: tigerstyle-reviewer

## Proposed Solutions

### Option 1: Add iteration counter with assertion

**Approach:** Add `let iterations = 0` and `assert(++iterations <= MAX_BASE58_LENGTH)` inside the loop.

**Pros:**
- Makes the bound explicit and machine-checked
- Zero cost in practice (22 iterations max)
- Documents the invariant

**Cons:**
- Slightly verbose

**Effort:** 5 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts` — `encodeBase58` function, while loop

## Acceptance Criteria

- [ ] Loop has explicit iteration bound assertion
- [ ] Assertion uses `MAX_BASE58_LENGTH` constant
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified unbounded loop pattern per TigerStyle rules
- Confirmed mathematical bound of 22 iterations
