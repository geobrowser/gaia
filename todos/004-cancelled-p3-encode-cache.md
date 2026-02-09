---
status: cancelled
priority: p3
issue_id: "004"
tags: [code-review, performance, base58]
dependencies: []
---

# Add bounded cache to encodeBase58 for hot-path performance

## Problem Statement

The GraphQL `serialize` path now calls `encodeBase58` for every UUID field in every response. This uses BigInt division (~22 iterations, ~50 allocations per call). The old path was `replaceAll("-", "")` which is ~11.6x faster. While the absolute cost is low (~5.5 us/op), responses with many entities and repeated UUIDs (type IDs, space IDs) would benefit from caching.

**Impact:** For 100 entities x 5 UUIDs = 500 calls: ~2.7ms added latency. For 1000 entities x 10 UUIDs: ~55ms. Not a blocker but worth watching.

## Findings

- Performance benchmarks show ~11.6x slowdown vs old path (performance-oracle)
- Knowledge graph responses reuse the same UUIDs heavily (type IDs, space IDs, property IDs appear on every entity)
- A bounded Map cache with 2048 entries (~300KB) would achieve ~8-24x speedup for cache hits
- The `isBase58` check on the `toUuid` serialize path is a non-issue — dashed hex from DB matches on the first regex test and never reaches Base58 branch
- `BASE58_DECODE_MAP` (128 bytes) is already L1-cache optimal

## Proposed Solutions

### Option 1: Bounded Map cache in encodeBase58

**Approach:** Add a `Map<string, string>` cache with FIFO eviction at 2048 entries.

**Pros:**
- Simple, ~15 lines of code
- Eliminates BigInt work for repeated UUIDs
- 300KB bounded memory

**Cons:**
- Module-level mutable state
- FIFO isn't optimal (LRU would be better but more complex)

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Monitor and defer

**Approach:** Add no cache now. Monitor latency in production. Add cache if needed.

**Pros:**
- No added complexity
- Current performance is acceptable for typical response sizes

**Cons:**
- If a heavy response pattern emerges, latency spikes before fix

**Effort:** 0

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts` — encodeBase58 function

## Acceptance Criteria

- [ ] If cache added: encodeBase58 benchmark improves 5x+ for repeated inputs
- [ ] If cache added: memory bounded at configured limit
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Performance Oracle Review

**By:** Claude Code (performance-oracle agent)

**Actions:**
- Benchmarked encode/decode at ~5.5us/op and ~3.8us/op respectively
- Estimated impact for typical response sizes (10-500 entities)
- Confirmed BigInt loop is bounded at 22 iterations (O(1) for 128-bit)
- Cache would primarily help GraphQL responses where same type/space UUIDs repeat
