---
status: wontfix
priority: p3
issue_id: "006"
tags: [code-review, performance, safety, proposals]
dependencies: []
---

# Add query timeout for active proposal check endpoints

## Problem Statement

The `hasActiveProposalForTarget` query has no statement-level timeout. If indexes are missing or stale, the query could run indefinitely. While the query is currently lightweight, explicit timeouts are a safety net against pathological cases.

## Findings

- No `statement_timeout` set at query level or connection pool level for these endpoints
- The connection pool has `connectionTimeoutMillis: 3000` and `idleTimeoutMillis: 30000` but no query timeout
- The query should complete in < 10ms under normal conditions
- This applies to all queries in the module, not just the new ones

## Proposed Solutions

### Option 1: Set statement_timeout at pool level

**Approach:** Configure a default `statement_timeout` in the connection pool options.

**Pros:**
- Covers all queries, not just new ones
- Single configuration point

**Cons:**
- May affect long-running queries elsewhere

**Effort:** 15 minutes

**Risk:** Low (if timeout is generous, e.g., 30s)

## Recommended Action

_To be filled during triage._

## Technical Details

**Affected files:**
- Database connection pool configuration
- `api/src/proposals/queries.ts` (all query functions)

## Resources

- **PR:** #400
- **Review agents:** tigerstyle-reviewer, performance-oracle

## Acceptance Criteria

- [x] Query timeout configured (pool-level or statement-level) — **already handled at database level**
- [x] Timeout is generous enough for normal operations (e.g., 30s)
- [x] Existing tests pass

## Work Log

### 2026-02-13 - Initial Discovery

**By:** Claude Code (multi-agent review)

**Actions:**
- Verified no query timeout exists at any level
- Confirmed connection pool has connection/idle timeouts but not statement timeout

### 2026-02-13 - Wontfix

**By:** Claude Code

**Reason:** Statement timeout is already configured at the database level directly. No application-side change needed.
