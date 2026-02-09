---
status: complete
priority: p2
issue_id: "025"
tags: [code-review, documentation]
dependencies: []
---

# Stale JSDoc in `base58UuidPlugin.ts` claims multi-format input

## Problem Statement

The module-level JSDoc comment in `base58UuidPlugin.ts` says the plugin "accepts UUID inputs in any format (dashed hex, undashed hex, Base58)" but the code now uses `fromBase58()` which only accepts Base58. Misleading documentation causes confusion for developers reading the code.

## Findings

- `api/src/kg/base58UuidPlugin.ts:31-33` — comment says "accepts UUID inputs in any format"
- `parseValue` (line ~15) and `parseLiteral` (line ~22) both call `fromBase58()` which rejects hex formats
- Comment was accurate before the Base58-only restriction was applied but wasn't updated

## Proposed Solutions

### Option 1: Update the JSDoc

**Approach:** Change the comment to reflect Base58-only input. Something like: "Accepts Base58-encoded UUID inputs. Serializes UUIDs as Base58."

**Pros:**
- Trivial fix, immediate value
- Prevents developer confusion

**Cons:**
- None

**Effort:** 5 minutes

**Risk:** None

## Acceptance Criteria

- [ ] JSDoc accurately describes Base58-only input behavior
- [ ] No references to "dashed hex" or "undashed hex" in the plugin file's documentation

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Found stale comment during review of Base58-only input restriction changes
- Confirmed `parseValue` and `parseLiteral` both use `fromBase58()` (Base58-only)

**Learnings:**
- When changing behavior at boundaries, always audit associated documentation/comments
