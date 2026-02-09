---
status: complete
priority: p3
issue_id: "018"
tags: [code-review, testing, graphql]
dependencies: []
---

# UUID scalar test should assert exact Base58 value

## Problem Statement

`uuidScalarPlugin.test.ts` checks result doesn't contain dashes and is ≤22 chars, but doesn't assert the exact expected Base58 value. Asserting exact values provides stronger regression protection.

## Findings

- Current test uses shape assertions (no dashes, length ≤22) instead of exact value assertions
- With known input UUIDs, the exact Base58 output is deterministic and can be asserted
- Found by: code-reviewer

## Proposed Solutions

### Option 1: Add exact value assertions

**Approach:** For each known test UUID, compute and assert the exact Base58 value.

**Effort:** 15 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/kg/uuidScalarPlugin.test.ts`

## Acceptance Criteria

- [ ] At least one test asserts exact Base58 output for a known UUID input
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified weak assertions in UUID scalar tests
