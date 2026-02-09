---
status: complete
priority: p3
issue_id: "028"
tags: [code-review, security, base58]
dependencies: []
---

# `encodeBase58` error echoes up to 40 chars of input content

## Problem Statement

The precondition error in `encodeBase58` includes up to 40 characters of the input string in the error message. While this function is only reachable from the output path (not directly from user input), defense-in-depth suggests error messages should not echo content.

## Findings

- `api/src/utils/base58.ts:44-46` — error message includes `input.slice(0, 40)` 
- `encodeBase58` is called from `toBase58()` which is the output serialization path
- Not directly reachable from user input (input goes through `decodeBase58` / `fromBase58`)
- Low severity but inconsistent with the security convention established in commit `24c0a92`

## Proposed Solutions

### Option 1: Remove content echo, keep length only

**Approach:** Change the error message to include only `input.length` instead of the content slice. Matches the convention in `toUuid()` error messages.

**Pros:**
- Consistent with established convention
- No information leakage even in edge cases
- Trivial fix

**Cons:**
- Slightly less debugging info (but length is sufficient to diagnose)

**Effort:** 5 minutes

**Risk:** None

## Acceptance Criteria

- [ ] `encodeBase58` error message does not include input content
- [ ] Error message includes input length for debugging
- [ ] Existing tests pass

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Found content echo in `encodeBase58` precondition error
- Confirmed this is output-path only (low risk) but violates convention
- Verified convention was established in commit `24c0a92`

**Learnings:**
- Even output-path errors should follow the same conventions for consistency
