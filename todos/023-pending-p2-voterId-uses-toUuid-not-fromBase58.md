---
status: pending
priority: p2
issue_id: "023"
tags: [code-review, base58, input-boundary]
dependencies: []
---

# `voterId` in `buildProposalResponse` uses `toUuid` instead of `fromBase58`

## Problem Statement

The `voterId` query parameter in the proposals router is parsed with `toUuid()` (the internal multi-format parser) instead of `fromBase58()` (the Base58-only input boundary function). This violates the convention established in this PR where all API input boundaries use `fromBase58()`.

It works today because `toUuid()` accepts Base58, but it silently accepts dashed hex and dashless hex — formats we've explicitly decided to reject at the API boundary.

## Findings

- `api/src/proposals/router.ts:225-228` — `buildProposalResponse` receives `voterId` as a raw string from query params and calls `toUuid(voterId)`.
- The `voterId` is a user-facing query parameter, making it an input boundary that should use `fromBase58()`.
- All other query/path parameters in the same file already use `fromBase58()` or `isValidBase58Id()`.

## Proposed Solutions

### Option 1: Decode `voterId` at the input boundary

**Approach:** Move `voterId` decoding to the route handler (around line ~383 where the query param is extracted), convert with `fromBase58()`, and pass the resulting `Uuid` into `buildProposalResponse`. Remove the `toUuid` call inside `buildProposalResponse`.

**Pros:**
- Consistent with every other input boundary in the codebase
- Validates Base58-only at the edge, matching API contract

**Cons:**
- None significant

**Effort:** 15 minutes

**Risk:** Low

## Acceptance Criteria

- [ ] `voterId` query param is decoded with `fromBase58()` at the route handler level
- [ ] `buildProposalResponse` receives a `Uuid` (not a raw string) for `voterId`
- [ ] Invalid (non-Base58) `voterId` returns 400 error
- [ ] Existing tests pass
- [ ] New test: non-Base58 `voterId` is rejected

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Identified during code review that `voterId` path doesn't follow the `fromBase58()` convention
- Confirmed all other input params in proposals router use `fromBase58()`/`isValidBase58Id()`
- Verified `toUuid()` still accepts hex formats, making this a convention violation not a bug

**Learnings:**
- Easy to miss individual params when doing bulk migration — `voterId` is extracted deeper in the handler chain
