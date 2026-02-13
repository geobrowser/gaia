---
status: done
priority: p2
issue_id: "002"
tags: [code-review, architecture, proposals, sql]
dependencies: []
---

# Extract SQL status conditions into shared fragment builders

## Problem Statement

Proposal status computation logic now lives in 3 places: `computeProposalStatus()` in TypeScript, `listProposalsInSpace` SQL conditions, and `hasActiveProposalForTarget` SQL conditions. The WARNING comments remind developers to keep them in sync, but this is a process control, not an engineering control. A future smart contract change (new voting mode, new threshold formula) requires updating all three in lockstep.

## Findings

- Status SQL conditions appear in `queries.ts` lines 478-499 (hasActiveProposalForTarget) and lines 583-641 (listProposalsInSpace)
- TypeScript reference implementation in `status.ts` lines 25-85
- WARNING comments at lines 273-275, 410-412, 454 acknowledge the risk
- Parity tests (queries.test.ts:375-617) only verify the TypeScript implementation, not the SQL
- 5 of 8 review agents flagged this as the most significant architectural concern

## Proposed Solutions

### Option 1: Extract SQL fragment builder functions

**Approach:** Create `sqlIsProposed(nowSeconds)`, `sqlIsExecutable(nowSeconds)`, `sqlIsRejected(nowSeconds)`, `sqlIsAccepted()` functions that return Drizzle SQL fragments. Both `listProposalsInSpace` and `hasActiveProposalForTarget` compose from these.

**Pros:**
- Reduces 3-way duplication to 2-way (SQL fragments + TypeScript reference)
- Single place to update SQL status logic
- Composable — `hasActiveProposalForTarget` uses `sqlIsProposed OR sqlIsExecutable`

**Cons:**
- SQL fragments are a new pattern in this codebase
- TypeScript and SQL implementations can still drift

**Effort:** 1-2 hours

**Risk:** Low

---

### Option 2: Add property-based integration test for SQL/TS parity

**Approach:** Insert proposals with random vote counts/thresholds/times into a test database, query them with SQL status filters, compute status with `computeProposalStatus()`, and assert both agree.

**Pros:**
- Catches drift between SQL and TypeScript automatically
- Doesn't require restructuring existing code

**Cons:**
- Requires test database setup (may already exist via `db:setup`)
- Doesn't reduce the duplication itself

**Effort:** 2-3 hours

**Risk:** Low

---

### Option 3: Both — extract fragments AND add integration tests

**Approach:** Combine options 1 and 2.

**Pros:**
- Maximum safety: structural prevention + automated detection

**Cons:**
- More effort upfront

**Effort:** 3-4 hours

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- `api/src/proposals/queries.ts:478-499, 583-641` — SQL status conditions
- `api/src/proposals/status.ts:25-85` — TypeScript reference
- `api/src/proposals/__tests__/queries.test.ts` — parity tests

## Resources

- **PR:** #400
- **Review agents:** code-reviewer, architecture-strategist, security-sentinel, data-integrity-guardian, tigerstyle-reviewer

## Acceptance Criteria

- [x] SQL status conditions exist in one place (fragment builders)
- [x] Both `listProposalsInSpace` and `hasActiveProposalForTarget` compose from shared fragments
- [x] All 52 existing tests pass
- [ ] (If Option 2/3) Integration test verifies SQL matches TypeScript for all status transitions

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Mapped all 3 locations where status logic is duplicated
- Verified WARNING comments acknowledge the risk
- Confirmed test suite only covers TypeScript side

**Learnings:**
- The smart contract logic is the source of truth
- Drizzle `sql` template supports composable fragments via interpolation
- The existing partial index `WHERE executed_at IS NULL` is designed around this status logic

### 2026-02-13 - Completed (Option 1)

**By:** Claude Code

**Actions:**
- Created `sqlIsAccepted()`, `sqlIsProposed(nowSeconds)`, `sqlIsExecutable(nowSeconds)`, `sqlIsRejected(nowSeconds)` fragment builders
- Updated both `hasActiveProposalForTarget` and `listProposalsInSpace` to compose from shared fragments
- Reduced SQL status logic from 3 independent copies to 1 shared set of builders
- All 52 tests pass, TypeScript clean
