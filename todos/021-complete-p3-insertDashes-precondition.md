---
status: complete
priority: p3
issue_id: "021"
tags: [code-review, safety, tigerstyle, uuid]
dependencies: []
---

# insertDashes should assert precondition (input length === 32)

## Problem Statement

`insertDashes()` in `uuid.ts` currently only asserts the postcondition (result length === 36). TigerStyle prefers explicit preconditions — asserting that the input is exactly 32 hex chars before processing.

## Findings

- `insertDashes` was extracted in commit 3 with a postcondition assertion
- Missing matching precondition: `assert(hex.length === 32)`
- All callers currently pass valid 32-char hex, but the function is exported
- Found by: tigerstyle-reviewer

## Proposed Solutions

### Option 1: Add precondition assertion

**Approach:** Add `assert(hex.length === 32, ...)` as the first line.

**Effort:** 5 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/uuid.ts` — `insertDashes` function

## Acceptance Criteria

- [ ] `insertDashes` asserts input length === 32
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified missing precondition per TigerStyle rules
