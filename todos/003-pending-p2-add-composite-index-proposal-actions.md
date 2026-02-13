---
status: pending
priority: p2
issue_id: "003"
tags: [code-review, performance, database, proposals]
dependencies: []
---

# Add composite index on proposal_actions for active proposal queries

## Problem Statement

The `hasActiveProposalForTarget` query uses a correlated EXISTS subquery on `proposal_actions` filtering by `(proposal_id, action_type, target_id)`, but only single-column indexes exist for `proposal_id` and `action_type`. PostgreSQL will use the `proposal_id` index and sequentially filter the remaining columns. This is fine at current scale but degrades as proposal counts grow.

## Findings

- Current indexes: `proposal_actions_proposal_id_idx` (btree on proposal_id), `proposal_actions_action_type_idx` (btree on action_type)
- No composite index covering the 3-column filter pattern
- The EXISTS subquery runs once per candidate proposal row in the outer query
- At 10K+ active proposals per space, the query time without a composite index could reach ~50ms vs ~10ms with it
- 3 of 8 review agents flagged this independently (performance, security, data-integrity)

## Proposed Solutions

### Option 1: Add composite index via migration

**Approach:** Create a Drizzle migration adding:
```sql
CREATE INDEX "proposal_actions_proposal_action_target_idx"
ON "proposal_actions" USING btree ("proposal_id", "action_type", "target_id");
```

**Pros:**
- Turns correlated subquery into index-only scan
- Trivial to add, zero risk to existing queries
- Benefits both new endpoints and future queries with this pattern

**Cons:**
- Slightly increases write overhead (one more index to maintain on INSERT)
- May not be needed yet at current scale

**Effort:** 15 minutes (migration file)

**Risk:** Low

---

### Option 2: Monitor first, add when needed

**Approach:** Deploy without the index, monitor query latency via OpenTelemetry spans, add the index if p99 latency exceeds threshold.

**Pros:**
- Avoids premature optimization
- Data-driven decision

**Cons:**
- Requires monitoring infrastructure
- Users experience degraded performance before fix

**Effort:** 0 now, 15 minutes later

**Risk:** Low

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- New migration file needed
- `api/src/proposals/queries.ts:470-476` — the EXISTS subquery that benefits

**Database changes:**
- New index: `proposal_actions(proposal_id, action_type, target_id)`
- No data migration, no schema change

## Resources

- **PR:** #400
- **Review agents:** performance-oracle, security-sentinel, data-integrity-guardian

## Acceptance Criteria

- [ ] Migration creates the composite index
- [ ] `EXPLAIN ANALYZE` confirms index-only scan on the EXISTS subquery
- [ ] Existing tests pass (no behavioral change)

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Identified missing composite index for 3-column filter
- Estimated scaling impact at 10x/100x proposal counts
- Confirmed existing partial index on proposals table is adequate

**Learnings:**
- The partial index `WHERE executed_at IS NULL` on proposals is already optimized for this query pattern
- The `SELECT EXISTS` short-circuits on first match, which mitigates the impact
- Most proposals have 1-5 actions, so current impact is minimal
