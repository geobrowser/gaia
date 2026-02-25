---
status: done
priority: p3
issue_id: "005"
tags: [code-review, quality, testing, proposals]
dependencies: []
---

# Parameterize duplicate active proposal test suites

## Problem Statement

The test file has two nearly identical `describe` blocks (~80 lines each) for the member and editor active proposal endpoints. Adding a new test case requires remembering to add it in both places.

## Findings

- `queries.test.ts:627-703` (member tests) and `queries.test.ts:705-783` (editor tests) share ~90% identical structure
- Only differences: URL path, parameter name in error messages, test constants
- Same 6 test cases repeated for each endpoint

## Proposed Solutions

### Option 1: Extract parameterized test helper

**Approach:** Create `describeActiveProposalEndpoint(label, buildUrl, targetLabel)` that generates the full test suite from parameters.

**Pros:**
- ~80 lines saved
- New test cases automatically apply to both endpoints
- Makes the symmetry explicit

**Cons:**
- Slightly harder to read individual test cases

**Effort:** 20 minutes

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- `api/src/proposals/__tests__/queries.test.ts:627-783`

## Resources

- **PR:** #400
- **Review agents:** code-simplicity-reviewer, pattern-recognition-specialist

## Acceptance Criteria

- [x] Both endpoints still have full test coverage
- [x] Test helper is parameterized, not duplicated
- [x] All 52 tests pass

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Identified symmetric test structure across both endpoint test suites
- Confirmed all 6 test cases are identical except for URL and error message string

### 2026-02-13 - Completed

**By:** Claude Code

**Actions:**
- Extracted `describeActiveProposalEndpoint()` helper that generates full test suite from parameters
- Both member and editor test suites now call this with their specific config (segment, targetLabel)
- Updated zero-rows test to expect 500 (matching SELECT EXISTS contract assertion)
- Reduced ~160 lines of duplicate tests to a single parameterized helper
- All 52 tests pass
