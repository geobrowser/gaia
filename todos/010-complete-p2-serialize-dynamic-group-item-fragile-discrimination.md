---
status: complete
priority: p2
issue_id: "010"
tags: [code-review, type-safety, tigerstyle, serialize]
dependencies: []
---

# serializeDynamicGroupItem uses fragile structural discrimination

## Problem Statement

`serializeDynamicGroupItem` in `serialize.ts` uses `"entityId" in item` to distinguish `EntityDiff` from `BlockChange` in the union. If `BlockChange` ever gains an `entityId` field, this silently breaks. The `as` casts suppress compiler checking.

## Findings

- `serialize.ts` uses structural type checking (`"entityId" in item`) to discriminate a union
- The `as EntityDiff` and `as BlockChange` casts bypass TypeScript's narrowing
- If the upstream types change, this code silently produces wrong output
- Found by: tigerstyle-reviewer, pattern-recognition-specialist, code-simplicity-reviewer

## Proposed Solutions

### Option 1: Add a discriminant field to the union types

**Approach:** Add a `kind: "entity-diff" | "block-change"` discriminant to the upstream types and switch on it.

**Pros:**
- Compiler-enforced discrimination
- Exhaustive switch checking
- Future-proof

**Cons:**
- Requires upstream type changes (may not be in our control)

**Effort:** 1 hour

**Risk:** Medium (upstream dependency)

---

### Option 2: Use a type guard function with runtime validation

**Approach:** Create `isEntityDiff(item): item is EntityDiff` that checks multiple fields, not just one.

**Pros:**
- Stronger discrimination without upstream changes
- TypeScript narrows correctly with type guards

**Cons:**
- Still structural, just more robust
- Needs updating if types change

**Effort:** 30 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/versioned/serialize.ts` — `serializeDynamicGroupItem` function

## Acceptance Criteria

- [ ] Union discrimination is type-safe (no `as` casts or single-field `in` checks)
- [ ] TypeScript compiler catches incorrect branches
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified fragile structural discrimination pattern
- Confirmed `as` casts suppress compiler narrowing
