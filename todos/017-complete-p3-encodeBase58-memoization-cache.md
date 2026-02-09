---
status: complete
priority: p3
issue_id: "017"
tags: [code-review, performance, base58, cache]
dependencies: []
---

# Add memoization cache to encodeBase58()

## Problem Statement

Same UUIDs appear repeatedly in responses (shared spaceId, recurring type IDs). A bounded `Map<string, string>` cache would eliminate 85-95% of BigInt work. Measured ~113x speedup on realistic entities.

**Note:** This was previously tracked as todo 004 (cancelled as premature optimization). The performance-oracle recommends it now based on profiling data showing repeated UUID patterns in typical API responses.

## Findings

- Performance benchmarks show ~5.5μs/op for encodeBase58
- Typical responses repeat spaceId, typeIds across many entities
- Bounded cache (2048 entries, ~300KB) achieves ~113x speedup for cache hits
- FIFO eviction is simplest; LRU would be better but more complex
- Found by: performance-oracle

## Proposed Solutions

### Option 1: Bounded FIFO Map cache

**Approach:** Module-level `Map<string, string>` with FIFO eviction at 2048 entries.

**Pros:**
- Simple (~15 lines), bounded memory
- Eliminates BigInt work for repeated UUIDs

**Cons:**
- Module-level mutable state

**Effort:** 30 minutes

**Risk:** Low

---

### Option 2: Defer until production metrics show need

**Approach:** Monitor latency, add cache if needed.

**Effort:** 0

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/base58.ts` — `encodeBase58` function

## Acceptance Criteria

- [ ] If cache added: benchmark improves 5x+ for repeated inputs
- [ ] If cache added: memory bounded at configured limit
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review, performance-oracle)

**Actions:**
- Profiled encodeBase58 at ~5.5μs/op
- Estimated cache hit rate of 85-95% for typical responses
- Previously cancelled as todo 004; re-raised with profiling data
