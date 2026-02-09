---
status: pending
priority: p3
issue_id: "029"
tags: [code-review, dead-code, search]
dependencies: []
---

# Redundant `MAX_SPACE_ID_LENGTH` check in search router

## Problem Statement

The search router defines `MAX_SPACE_ID_LENGTH = 22` and checks `space_id.length > MAX_SPACE_ID_LENGTH` before calling `isValidBase58Id()`. This length check is fully redundant because `isValidBase58Id()` already rejects strings longer than 22 characters (Base58-encoded UUIDs are at most 22 chars, and `decodeBase58` has a length guard).

## Findings

- `api/src/search/index.ts:46` — `const MAX_SPACE_ID_LENGTH = 22`
- `api/src/search/index.ts:280-287` — length check before `isValidBase58Id()` call
- `isValidBase58Id()` → `fromBase58()` → `decodeBase58()` which rejects length > 22
- The constant and check are dead code that adds visual noise

## Proposed Solutions

### Option 1: Remove the constant and length check

**Approach:** Delete `MAX_SPACE_ID_LENGTH` and the associated `if` block. Let `isValidBase58Id()` handle all validation.

**Pros:**
- Less code, less noise
- Single source of truth for validation (the Base58 codec)
- No behavior change

**Cons:**
- None (validation is fully covered by `isValidBase58Id`)

**Effort:** 10 minutes

**Risk:** None

## Acceptance Criteria

- [ ] `MAX_SPACE_ID_LENGTH` constant removed
- [ ] Length check removed from search router
- [ ] `isValidBase58Id()` still correctly rejects long strings (covered by existing tests)
- [ ] Existing tests pass

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Identified redundant length check that duplicates `isValidBase58Id` validation
- Confirmed `decodeBase58` has its own length guard (rejects > 22 chars)
- Traced the `MAX_SPACE_ID_LENGTH` constant origin — was 36 (UUID length) before Base58 migration, reduced to 22 but now fully redundant

**Learnings:**
- When migrating validation logic, check for redundant guards left from the old approach
