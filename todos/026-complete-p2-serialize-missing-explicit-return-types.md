---
status: complete
priority: p2
issue_id: "026"
tags: [code-review, type-safety, serialization]
dependencies: []
---

# `serialize.ts` serializers lack explicit return types

## Problem Statement

The serialization functions in `serialize.ts` use spread syntax to copy all fields and then override UUID fields with Base58. If a new `Uuid` field is added to a source type, it will silently pass through as dashed hex in the serialized output — the compiler won't catch it.

Explicit return types would cause a type error if a new UUID field isn't mapped to Base58.

## Findings

- `api/src/versioned/serialize.ts` — all public serializer functions (`serializeEntity`, `serializeVersion`, `serializeEntityDiff`, `serializeGroupedEntityDiff`) use `{ ...source, field: toBase58(source.field) }` pattern
- Return types are inferred, not declared
- Adding a new `Uuid` field to the source type would compile cleanly but leak hex in the response

## Proposed Solutions

### Option 1: Add explicit return types with Base58-branded string fields

**Approach:** Define return types where UUID fields are typed as `string` (or a `Base58` branded type) instead of `Uuid`. The spread would fail to compile if a `Uuid` field isn't explicitly overridden.

**Pros:**
- Compiler catches missing Base58 conversions
- Documents the serialization contract
- Defense-in-depth for future field additions

**Cons:**
- More type definitions to maintain
- Need to decide: inline types vs separate type declarations

**Effort:** 1-2 hours

**Risk:** Low

---

### Option 2: Use a mapped type utility

**Approach:** Create a `Serialized<T>` utility type that automatically maps `Uuid` fields to `string`, then use it as the return type.

**Pros:**
- Single utility handles all serializers
- Less manual type maintenance

**Cons:**
- More complex type machinery
- Harder to understand at a glance

**Effort:** 1-2 hours

**Risk:** Low

## Acceptance Criteria

- [ ] All public serializer functions have explicit return types
- [ ] Adding a new `Uuid` field to a source type without updating the serializer causes a compile error
- [ ] Existing tests pass

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Identified that all serializers in `serialize.ts` rely on type inference
- Verified that adding a `Uuid` field to source types would silently leak hex
- Evaluated two approaches: explicit types vs mapped type utility

**Learnings:**
- Spread-based serialization is convenient but fragile without explicit return types
- This is a common pattern in TypeScript codebases — worth establishing the convention early
