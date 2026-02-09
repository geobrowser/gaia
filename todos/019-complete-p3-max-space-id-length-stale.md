---
status: complete
priority: p3
issue_id: "019"
tags: [code-review, cleanup, search]
dependencies: []
---

# MAX_SPACE_ID_LENGTH = 36 in search module is stale

## Problem Statement

Base58 UUIDs are ≤22 chars, but `MAX_SPACE_ID_LENGTH` is still set to 36 (the old dashed hex length). The value 36 still works (22 < 36) but the intent is misleading. Should be updated or removed since `isValidUuid` handles validation.

## Findings

- `search/index.ts` defines `MAX_SPACE_ID_LENGTH = 36`
- With Base58 input, max length is 22 characters
- The constant still functions correctly (overly permissive, not broken)
- `isValidUuid()` already validates UUID format regardless of this length check
- Found by: architecture-strategist

## Proposed Solutions

### Option 1: Update to 22 or remove

**Approach:** Update constant to 22 (Base58 max) or remove it entirely since `isValidUuid` is the real validation.

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/search/index.ts` — `MAX_SPACE_ID_LENGTH` constant

## Acceptance Criteria

- [ ] Constant updated to correct value or removed
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified stale constant value
- Confirmed `isValidUuid` provides the real validation
