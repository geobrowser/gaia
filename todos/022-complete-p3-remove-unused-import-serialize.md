---
status: complete
priority: p3
issue_id: "022"
tags: [code-review, cleanup, serialize]
dependencies: []
---

# Remove GroupedEntitySnapshot unused import in serialize.ts

## Problem Statement

`serialize.ts` imports `GroupedEntitySnapshot` but doesn't use it. Dead import should be removed.

## Findings

- `GroupedEntitySnapshot` is imported but not referenced in `serialize.ts`
- Likely left over from a refactor
- Found by: code-reviewer

## Proposed Solutions

### Option 1: Remove the import

**Approach:** Delete the unused import line.

**Effort:** 2 minutes

**Risk:** None

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/versioned/serialize.ts` — import statement

## Acceptance Criteria

- [ ] Unused import removed
- [ ] Build passes (no type errors)

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified dead import
