---
status: complete
priority: p2
issue_id: "008"
tags: [code-review, api-consistency, base58, proposals]
dependencies: []
---

# Proposal cursor leaks dashed hex format

## Problem Statement

`proposals/queries.ts` `extractCursorValue` builds cursor as `${row.created_at}|${row.id}` where `row.id` is dashed hex. All other UUID output is Base58, but cursors contain dashed hex. Inconsistent but functional — clients parsing cursors would see a different UUID format than what's in the response body.

## Findings

- `extractCursorValue` in `proposals/queries.ts` concatenates the raw `row.id` (dashed hex from PostgreSQL) into the cursor string
- Cursor is an opaque pagination token, so clients shouldn't parse it — but it's inconsistent with the "Base58 at boundaries" principle
- Found by: code-reviewer, architecture-strategist

## Proposed Solutions

### Option 1: Encode cursor UUID as Base58

**Approach:** Change cursor construction to use `toBase58(toUuid(row.id))` and update cursor parsing to accept Base58 input via `toUuid()`.

**Pros:**
- Consistent: all UUIDs crossing the API boundary are Base58
- Cursor parsing already needs to handle UUID — `toUuid()` handles both formats

**Cons:**
- Cursors are opaque tokens — clients shouldn't care about format
- Minor: existing client cursors would break (pagination reset)

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Leave as-is, document as opaque

**Approach:** Cursors are opaque tokens by design. Document that cursor format is internal and should not be parsed.

**Pros:**
- No code change
- Cursors are meant to be opaque

**Cons:**
- Inconsistency remains visible in API responses

**Effort:** 0

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/proposals/queries.ts` — `extractCursorValue` function

## Acceptance Criteria

- [ ] Cursor format is consistent with API UUID format OR documented as intentionally opaque
- [ ] Existing pagination tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified cursor format inconsistency
- Confirmed cursors are functional despite format mismatch
