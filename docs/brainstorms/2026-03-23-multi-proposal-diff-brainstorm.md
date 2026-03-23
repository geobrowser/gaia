# Multi-Proposal Diff Groups

**Date:** 2026-03-23
**Status:** Brainstorm complete

## What We're Building

Support diffing multiple related proposals as one logical group instead of only diffing one proposal at a time.

Today, the proposal diff flow is singular: it loads one proposal, resolves one `Publish` action to one `content_uri`, fetches one blob from `ipfs_cache`, decodes one GRC-20 edit, extracts affected entities, and compares those changes against either live KG state or versioned state. A grouped diff should keep the same entity-centered output shape, but let callers provide multiple proposal IDs or edit IDs and see the combined effect as one review surface.

The important behavior choice is that this should behave like a single change set, not like multiple unrelated diffs rendered side by side. That means the grouped diff needs one shared base state and a deterministic order for applying edits so later proposals can build on or override earlier ones. The chosen ordering is by edit timestamp, so the grouped diff matches the real temporal sequence of the underlying edits rather than arbitrary request order.

## Approaches

### Approach 1: Request-Time Squash

Accept multiple proposal IDs or edit IDs, load all referenced edits in the handler, decode them, concatenate the ops in a deterministic order, and run the existing diff flow once over the squashed op list.

**Pros:**
- Reuses the current proposal diff logic with the least product and system change.
- Fastest path to validating the grouped-diff UX and semantics.
- Keeps indexing and backfill risk out of the first iteration.

**Cons:**
- Work scales linearly with group size because every request re-fetches and re-decodes every edit blob.
- Hot paths still pay the cost of relation lookup and op extraction on every request.
- Large groups will increase handler latency and memory pressure.

**Best when:** grouped diffs are new, group sizes are modest, and the priority is shipping correct semantics quickly.

### Approach 2: Indexed Proposed Ops

Pre-decode proposal edits during ingestion and store normalized proposal ops in Postgres so the diff handler can query affected entities and ordered ops directly instead of loading blobs from `ipfs_cache` and decoding in memory.

**Pros:**
- Avoids repeated blob fetch/decode work on every grouped diff request.
- Makes affected-entity discovery and pagination cheaper because they can be read from indexed rows.
- Creates a foundation for other proposal-analysis features beyond diffing.

**Cons:**
- Adds new storage, backfill, and consistency responsibilities.
- Requires deciding how much op payload to persist: full normalized ops, entity/relation touches only, or both.
- Raises lifecycle questions when proposal content changes or cached blobs are missing/corrupt.

**Best when:** grouped diffs will be frequent, proposal groups may be large, or proposal analysis is becoming a broader product surface.

## Why This Approach

Recommend starting with **Approach 1** and treating **Approach 2** as a follow-on optimization if real usage justifies it.

The current diff code already batches base-state reads efficiently; the main singular bottlenecks are proposal lookup, blob fetch, decode, and affected-entity extraction. That means the simplest grouped design is not wasting obviously optimized infrastructure. It mostly extends the current behavior from "one edit" to "an ordered list of edits" and gives us a clean way to validate group semantics before committing to new tables and ingestion work.

If latency becomes a problem, the indexed-op design is a natural second step because it targets the specific repeated work that grouped diffs amplify.

## Key Decisions

1. **Model grouped diff as one ordered change set** rather than a bag of independent proposal diffs.
2. **Keep single-proposal diff intact** and add a grouped contract alongside it instead of overloading the existing singular API immediately.
3. **Order grouped edits by edit timestamp** so the combined diff reflects actual change chronology.
4. **Support active, closed, and executed groups**. Historical grouped diffs are in scope, not an active-only simplification.
5. **Require one `spaceId` per group**. Cross-space grouped diffs should stay out of scope.
6. **Prefer the simplest path first** because the existing base-state fetch path is already batched and the hardest unanswered questions are semantic, not infrastructural.

## Open Questions

1. **Input contract:** should the group API accept only proposal IDs, or both proposal IDs and edit IDs?
2. **Conflict visibility:** if proposal B overwrites proposal A in the same group, should the response expose that overwrite explicitly or only show the final combined diff?
3. **Historical base resolution:** for groups containing multiple closed/executed proposals, should the base snapshot anchor to the earliest edit timestamp in the group, or some proposal-level boundary?
4. **Index shape for the performance path:** do we need a full normalized op table, or would indexed affected entities plus ordered raw payloads be enough?

## Next Steps

→ `/workflows:plan` for endpoint shape, validation rules, ordering policy, and test coverage.
