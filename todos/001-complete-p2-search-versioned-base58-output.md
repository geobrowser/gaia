---
status: complete
priority: p2
issue_id: "001"
tags: [code-review, api-consistency, base58]
dependencies: []
---

# Search and versioned endpoints return dashed hex, not Base58

## Problem Statement

The `search/` and `versioned/` REST endpoints still return dashed hex UUIDs in their responses, while `profile/`, `proposals/`, and GraphQL all return Base58. This creates an inconsistent API surface — clients consuming multiple endpoints see different UUID formats.

**Impact:** Clients must handle two different ID formats depending on which endpoint they call.

## Findings

- **Search responses** (`services/search/opensearch.ts:138-154`): `SearchResult` objects are built directly from OpenSearch `_source` fields (`entityId`, `spaceId`, `typeIds`) which contain dashed hex. Returned as-is via `c.json(response)` in `search/index.ts:402`.
- **Versioned responses** (`versioned/queries.ts`, `versioned/router.ts`): All UUID fields in `EntitySnapshot`, `GroupedEntityDiff`, `PaginatedProposalDiff` are typed as `Uuid` (dashed hex). Returned directly via `c.json()`.
- **Integration tests** (`versioned/__tests__/integration.test.ts`): Were explicitly updated to assert dashed hex format (`expectDashedUuid`), suggesting this may have been intentional.
- Found by: code-reviewer, architecture-strategist, pattern-recognition-specialist (all 3 flagged independently)

## Proposed Solutions

### Option 1: Add Base58 encoding to search and versioned output boundaries

**Approach:** Map all UUID fields through `toBase58(toUuid(...))` at the serialization boundary in both modules.

**Pros:**
- Consistent API surface across all endpoints
- Clients only need one UUID format

**Cons:**
- Performance cost of Base58 encoding on potentially large versioned responses
- Versioned responses have deeply nested UUID fields (values, relations, blocks) — many conversion points

**Effort:** 2-4 hours

**Risk:** Medium (many fields to convert, risk of missing one)

---

### Option 2: Document the intentional divergence

**Approach:** Leave search/versioned as dashed hex. Document that these are "internal" endpoints with a different contract.

**Pros:**
- No code changes
- Versioned data stays in the format closest to storage

**Cons:**
- Inconsistent client experience
- Harder to reason about API contracts

**Effort:** 30 minutes

**Risk:** Low

---

### Option 3: Phase this as a follow-up PR

**Approach:** Merge current PR as-is (profile/proposals/GraphQL use Base58), then do search+versioned in a follow-up.

**Pros:**
- Smaller PRs, easier to review
- Can address versioned endpoint's deep nesting carefully

**Cons:**
- Temporary inconsistency in production

**Effort:** Same as Option 1, deferred

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/services/search/opensearch.ts:138-154` — SearchResult construction
- `api/src/search/index.ts:402` — Search response serialization
- `api/src/versioned/queries.ts` — All `toUuid()` calls for row mapping
- `api/src/versioned/router.ts:232` — `c.json(snapshot)` serialization
- `api/src/versioned/types.ts` — All `Uuid` typed fields

## Acceptance Criteria

- [ ] All REST endpoints return the same UUID format (Base58) OR divergence is documented
- [ ] Tests updated to assert correct output format
- [ ] No regressions in existing tests

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (7-agent review)

**Actions:**
- Identified inconsistency across 3 independent review agents
- Verified integration tests explicitly assert dashed hex for versioned endpoints
- Confirmed search results pass through OpenSearch values unconverted

**Learnings:**
- The versioned integration tests were updated to expect dashed hex, suggesting this may have been a deliberate scope decision during implementation
