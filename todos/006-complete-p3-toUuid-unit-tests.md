---
status: complete
priority: p3
issue_id: "006"
tags: [code-review, testing, base58]
dependencies: []
---

# Add unit tests for toUuid() disambiguation logic

## Problem Statement

There is no direct unit test for `toUuid()` with Base58 input. The Base58 roundtrip tests in `base58.test.ts` and the GraphQL scalar tests cover the path implicitly, but there's no explicit test for the format disambiguation logic (dashed hex wins over dashless, hex wins over Base58, ambiguous inputs).

**Impact:** If the disambiguation order breaks, the regression would only be caught by integration tests, not fast unit tests.

## Findings

- `uuid.ts` has no corresponding `uuid.test.ts` unit test file
- `isValidUuid()` is tested indirectly through other modules
- The disambiguation order (dashed → dashless → Base58) is critical and untested directly
- Edge case: a 32-char hex-only string should match as dashless hex, not Base58
- Edge case: `toUuid("2")` should decode Base58 → `00000000-0000-0000-0000-000000000001`

## Proposed Solutions

### Option 1: Create uuid.test.ts with disambiguation tests

**Approach:** Add tests for: dashed input, dashless input, Base58 input, mixed case, whitespace trimming, invalid input errors, and the hex-wins-when-ambiguous rule.

**Effort:** 30 minutes

**Risk:** None

## Recommended Action

*To be filled during triage.*

## Acceptance Criteria

- [ ] `toUuid()` tested with all 3 input formats
- [ ] Disambiguation order explicitly tested (hex wins for ambiguous strings)
- [ ] Error cases tested (invalid strings, empty, too long)
- [ ] `toBase58()` tested for known UUID → Base58 mappings

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (code-reviewer agent)

**Actions:**
- Identified missing uuid.test.ts file
- Confirmed Base58 path is only tested indirectly through uuidScalarPlugin.test.ts and base58.test.ts
