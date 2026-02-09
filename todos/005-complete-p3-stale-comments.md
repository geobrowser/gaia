---
status: complete
priority: p3
issue_id: "005"
tags: [code-review, documentation, cleanup]
dependencies: []
---

# Fix stale comments referencing old UUID format

## Problem Statement

Several comments still reference "dashless" or "undashed" format when the code now uses dashed hex internally and Base58 for output. These create confusion for developers reading the code.

## Findings

- **`versioned/__tests__/integration.test.ts:755-759`**: Block comment says "API must return dashless lowercase hex UUIDs (32 chars, no dashes)" and "catch regressions where Postgres dashed UUIDs leak through" — but the `describe` block and assertions were correctly updated to expect **dashed** UUIDs. The comment is exactly backwards now.
- **`proposals/types.ts:270`**: JSDoc says `/** Member space ID of the proposer (dashless UUID) */` — should say "Base58-encoded UUID" since `proposals/router.ts` applies `toBase58(toUuid(proposal.proposedBy))`.
- **`kg/__tests__/entitySpaceFilterPlugin.test.ts:42,56`**: Comments say "Convert UUID to undashed format for GraphQL input" — technically correct but could be clearer that this is one of several accepted input formats.

## Proposed Solutions

### Option 1: Fix all stale comments

**Approach:** Update the 3 comment sites to reflect current behavior.

**Effort:** 15 minutes

**Risk:** None

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/versioned/__tests__/integration.test.ts:755-759`
- `api/src/proposals/types.ts:270`
- `api/src/kg/__tests__/entitySpaceFilterPlugin.test.ts:42,56`

## Acceptance Criteria

- [ ] Integration test block comment describes dashed hex format (not dashless)
- [ ] proposals/types.ts JSDoc mentions Base58 or removes format detail
- [ ] No other stale format references remain

## Work Log

### 2026-02-09 - Pattern Recognition Review

**By:** Claude Code (pattern-recognition-specialist)

**Actions:**
- Searched all comments for "dashless", "undashed", "32 chars" references
- Identified 3 stale comment sites
- Confirmed code behavior is correct, only comments are wrong
