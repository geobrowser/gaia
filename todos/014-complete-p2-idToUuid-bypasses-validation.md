---
status: complete
priority: p2
issue_id: "014"
tags: [code-review, safety, tigerstyle, proposal-diff]
dependencies: []
---

# idToUuid in proposal-diff.ts bypasses toUuid validation

## Problem Statement

`idToUuid` in `proposal-diff.ts` constructs a `Uuid` directly from raw bytes without length check on `id` or assertion that the result matches the dashed UUID pattern. If `Id` is not exactly 16 bytes, it silently produces a malformed UUID branded as `Uuid`.

## Findings

- `idToUuid` takes raw `Id` bytes and converts to hex, then inserts dashes
- No validation that `id` is exactly 16 bytes
- No assertion that output matches UUID pattern
- Brands result as `Uuid` without going through `toUuid()` validation
- Found by: tigerstyle-reviewer

## Proposed Solutions

### Option 1: Add precondition and postcondition assertions

**Approach:** Assert `id.length === 16` at entry and assert result matches UUID pattern at exit. Or route through `toUuid()`.

**Pros:**
- Catches malformed input immediately
- Consistent with validated `Uuid` branded type
- Simple assertions

**Cons:**
- Minor performance cost (negligible)

**Effort:** 15 minutes

**Risk:** Low

---

### Option 2: Route through toUuid() for validation

**Approach:** Build the hex string and pass through `toUuid()` instead of manually inserting dashes and casting.

**Pros:**
- Single validation path for all UUID construction
- No duplicate dash-insertion logic

**Cons:**
- Slightly less efficient (regex parsing of known-good format)

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/versioned/proposal-diff.ts` — `idToUuid` function

## Acceptance Criteria

- [ ] `idToUuid` validates input byte length
- [ ] Output is verified as valid UUID before branding
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified unvalidated Uuid construction path
- Confirmed `Id` type doesn't guarantee 16-byte length
