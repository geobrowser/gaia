---
status: complete
priority: p2
issue_id: "024"
tags: [code-review, base58, performance]
dependencies: []
---

# Double-decode pattern: `isValidBase58Id()` + `fromBase58()` decodes Base58 twice

## Problem Statement

Every router handler follows this pattern:

```ts
if (!isValidBase58Id(id)) return c.json({ error: "..." }, 400);
const uuid = fromBase58(id);
```

Both `isValidBase58Id()` and `fromBase58()` internally call `decodeBase58()`, so every input UUID is decoded twice (~800ns wasted per UUID). With multiple UUID params per request, this adds up.

## Findings

- `api/src/utils/uuid.ts` — `isValidBase58Id()` calls `fromBase58()` internally, which calls `decodeBase58()`
- `fromBase58()` also calls `decodeBase58()`
- Pattern appears in ~39 call sites across all routers
- Each `decodeBase58()` call involves BigInt arithmetic — not free

## Proposed Solutions

### Option 1: Add `tryFromBase58(value: string): Uuid | null`

**Approach:** Create a new function that returns `null` on failure instead of throwing. Routers replace the two-call pattern with:

```ts
const uuid = tryFromBase58(id);
if (uuid === null) return c.json({ error: "..." }, 400);
```

**Pros:**
- Eliminates all double-decode sites
- Cleaner control flow (no exception for expected validation failures)
- Single function to maintain

**Cons:**
- One more function in the uuid module
- Need to update ~39 call sites

**Effort:** 1-2 hours

**Risk:** Low

---

### Option 2: Memoize `decodeBase58`

**Approach:** Add a small LRU/FIFO cache to `decodeBase58()` so the second call is a cache hit.

**Pros:**
- No call site changes needed
- Also helps other repeated decode paths

**Cons:**
- Adds caching complexity to a low-level function
- `encodeBase58` already has a cache — adding one to decode doubles cache management
- Doesn't fix the awkward two-call control flow

**Effort:** 30 minutes

**Risk:** Low

## Acceptance Criteria

- [ ] No input boundary decodes the same Base58 value twice
- [ ] Validation + parsing happens in a single function call
- [ ] All existing tests pass
- [ ] Performance: measurably fewer `decodeBase58` calls per request

## Work Log

### 2026-02-09 - Initial Discovery

**By:** Claude Code (8-agent parallel review)

**Actions:**
- Identified double-decode pattern across all router input boundaries
- Counted ~39 sites where `isValidBase58Id()` precedes `fromBase58()`
- Evaluated two approaches: `tryFromBase58` vs memoization

**Learnings:**
- The validate-then-parse pattern is a common source of redundant work
- `tryFromBase58` is the idiomatic fix — returns null instead of throwing for expected failures
