---
status: complete
priority: p2
issue_id: "009"
tags: [code-review, security, base58, tigerstyle]
dependencies: []
---

# decodeBase58 missing explicit length guard

## Problem Statement

`decodeBase58` is a public export that accepts strings of any length. While the overflow check catches invalid values, a maliciously long string (e.g. 10K chars) burns CPU in BigInt arithmetic before hitting the overflow guard. Should add an early length check at function entry.

## Findings

- `decodeBase58` in `base58.ts` iterates over every character doing BigInt multiplication before checking overflow
- Base58 encoding of a 128-bit UUID is at most 22 characters
- A 10K character input would perform ~10K BigInt multiply-add operations before overflow detection
- Found by: tigerstyle-reviewer, security-sentinel

## Proposed Solutions

### Option 1: Add max length guard at function entry

**Approach:** Add `if (encoded.length > MAX_BASE58_LENGTH) throw new Error(...)` as the first line of `decodeBase58`.

**Pros:**
- O(1) rejection of oversized input
- Prevents CPU burn from malicious strings
- Simple, one line of code

**Cons:**
- None meaningful

**Effort:** 10 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts` — `decodeBase58` function

## Acceptance Criteria

- [ ] `decodeBase58` rejects strings longer than 22 chars before doing BigInt work
- [ ] Test added for oversized input rejection
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified missing input length guard
- Confirmed MAX_BASE58_LENGTH = 22 is the correct bound for 128-bit UUIDs
