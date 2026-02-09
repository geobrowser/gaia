---
status: complete
priority: p2
issue_id: "011"
tags: [code-review, architecture, base58, profile]
dependencies: []
---

# Profile module uses Base58 as Map keys internally

## Problem Statement

`profile/queries.ts:75` keys the profile Map on `toBase58(toUuid(row.space_id))` instead of `Uuid`. This violates the "Uuid internally, Base58 at boundaries" principle. If you ever need the raw UUID for a join or DB query, you'd have to decode. The versioned module correctly uses `Uuid` keys.

## Findings

- `profile/queries.ts` line 75 uses `toBase58(toUuid(row.space_id))` as Map key
- The versioned module uses `Uuid` (dashed hex) as Map keys — the correct pattern
- Base58 in Map keys means any lookup by raw UUID would require encoding first
- Found by: architecture-strategist, pattern-recognition-specialist

## Proposed Solutions

### Option 1: Use Uuid as Map key, encode at serialization boundary

**Approach:** Change Map key to `toUuid(row.space_id)` and apply `toBase58()` only when building the response object.

**Pros:**
- Consistent with versioned module pattern
- Enables Uuid-based lookups
- Clear boundary between internal and external formats

**Cons:**
- Minor refactor

**Effort:** 20 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/profile/queries.ts:75` — Map key construction

## Acceptance Criteria

- [ ] Profile Map uses `Uuid` keys, not Base58
- [ ] Base58 encoding happens at response serialization only
- [ ] Profile endpoint tests pass with correct output format

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified violation of "Uuid internally" architecture principle
- Compared with versioned module which follows the correct pattern
