---
status: done
priority: p2
issue_id: "001"
tags: [code-review, quality, proposals, router]
dependencies: []
---

# Extract shared route handler factory for active proposal check endpoints

## Problem Statement

The two new active proposal check route handlers in `router.ts` are ~100 lines each and nearly identical, differing only in parameter name, validation message, query function, and log/span labels. This is a maintenance trap — any future bug fix or behavior change must be applied in both places, and a third endpoint of this shape (e.g., `RemoveMember`) would triple the duplication.

## Findings

- `router.ts:646-747` (member handler) and `router.ts:749-850` (editor handler) share ~95% of their code
- The only differences are 5 string/function values: path param name, validation message, query function, operation name prefix, and span name
- The existing codebase doesn't have a handler factory pattern, but the query layer already uses a shared implementation (`hasActiveProposalForTarget`)
- 6 of 8 review agents flagged this duplication

## Proposed Solutions

### Option 1: Extract a handler factory function

**Approach:** Create a `createActiveProposalRoute` function that accepts a config object (path, param name, label, query function, operation name) and registers the route.

**Pros:**
- Eliminates ~100 lines of duplication
- Future endpoints are a single config call
- Bug fixes apply once

**Cons:**
- Adds one level of indirection
- No existing handler factory pattern in codebase

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Keep as-is, revisit on third endpoint

**Approach:** Leave the duplication. If a third endpoint of this shape appears, extract then.

**Pros:**
- No indirection; each handler is self-contained
- Follows locality of behavior principle

**Cons:**
- Maintenance burden for two handlers
- Easy to forget updating both

**Effort:** 0

**Risk:** Low (for now)

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- `api/src/proposals/router.ts:646-850` — two route handlers

## Resources

- **PR:** #400
- **Review agents:** code-reviewer, code-simplicity-reviewer, pattern-recognition-specialist, architecture-strategist, tigerstyle-reviewer

## Acceptance Criteria

- [x] Both endpoints produce identical behavior after refactor
- [x] All 52 existing tests pass
- [x] No new abstractions leaked outside the router module

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Identified ~200 lines of near-identical route handler code
- Confirmed only 5 values differ between the two handlers
- Verified query layer already uses shared implementation pattern

**Learnings:**
- The codebase prefers explicit code over abstraction (locality of behavior)
- Two is acceptable; three is the refactoring trigger per architecture review

### 2026-02-13 - Completed

**By:** Claude Code

**Actions:**
- Extracted `registerActiveProposalRoute()` with `ActiveProposalRouteConfig` interface
- Both member and editor endpoints now call the shared handler with their specific config
- All 52 tests pass, TypeScript clean
