# Multi-Proposal Diff Groups (RFC)

*Date: 2026-03-23*

## Summary

We want proposal diffing to support a logical group of related proposals, not just a single proposal at a time.

Today, `GET /versioned/proposals/:id/diff` computes an on-demand diff for one proposal by loading its `Publish` action, fetching the edit blob from `ipfs_cache`, decoding the ops, extracting affected entities, and comparing the proposed state against either live KG state or versioned state. This RFC extends that model so callers can diff multiple proposals as one ordered change set.

The recommended first step is to keep the current single-proposal route unchanged and add a grouped diff contract that accepts multiple **proposal IDs** in one space. The grouped diff applies their edits in chronological order by **edit timestamp**, then returns one entity-centered diff payload. If grouped diff latency later becomes a problem, we can add a proposal-op index to avoid repeated blob fetch and decode work.

## Goals

- Support diffing multiple related proposals as one combined change set
- Preserve the existing single-proposal diff endpoint and semantics
- Order grouped edits by edit timestamp, not request order
- Support both active proposal groups and historical groups for closed/executed proposals
- Reuse the existing entity diff shape as much as possible

## Non-Goals

- Replacing the single-proposal diff route
- Supporting cross-space grouped diffs
- Supporting arbitrary raw `editId` input in the public v1 API
- Precomputing or indexing proposal ops in v1
- Solving mixed live-plus-historical base semantics in the first iteration

## Current State

The current proposal diff flow lives in `api/src/versioned/proposal-diff.ts` and is exposed by `api/src/versioned/router.ts` via `GET /versioned/proposals/:id/diff`.

The flow is:

1. Load one proposal and its `Publish` action
2. Validate the provided `spaceId`
3. Fetch one edit blob from `ipfs_cache`
4. Decode one GRC-20 edit into ops
5. Extract affected entities and relations
6. Fetch base state:
   - active proposals -> current live state
   - closed proposals -> versioned state at `end_time`
   - executed proposals -> versioned state just before `executed_at`
7. Apply ops in memory and diff base vs proposed state

This design works well for one proposal, but it does all proposal lookup, blob fetch, decode, and op extraction work on every request. That cost grows linearly if we want to diff multiple related proposals together.

## Proposal

### API Surface

Add a grouped diff contract alongside the existing singular route.

One acceptable shape is:

`GET /versioned/proposal-groups/diff?spaceId=<uuid>&proposalIds=<uuid>,<uuid>,...`

The exact path can be finalized during planning, but the important constraint is that the grouped contract is **additive**. The existing `GET /versioned/proposals/:id/diff` route stays unchanged.

The grouped contract should accept:

- one `spaceId`
- two or more `proposalIds`
- existing pagination inputs (`cursor`, `limit`)

The public v1 API should accept **proposal IDs only**.

### Why Proposal IDs, Not Edit IDs

Grouped diff semantics depend on proposal metadata, not just edit payloads:

- validating all inputs belong to one space
- determining whether the group is active or historical
- deciding which base-state mode to use
- preserving a product concept of "related proposals" rather than exposing a lower-level storage primitive

An internal edit-level primitive may still be useful later, but it should not be the first public contract.

### Ordering

Grouped proposals are applied in ascending order by the edit timestamp recorded in `edit_versions.created_at`.

This makes the grouped diff reflect the real change chronology of the underlying edits rather than client-supplied order or proposal lifecycle timestamps such as `start_time` or `created_at`.

### Base-State Semantics

Grouped diff should behave like one ordered change set applied to one shared base.

We define two base modes for v1:

#### 1) Active Group

If all proposals in the group are active, compute the base from the current live KG state, then apply all grouped edits in edit-timestamp order.

This matches the current single-proposal meaning of "what would change if these proposals executed now."

#### 2) Historical Group

If all proposals in the group are closed or executed, compute the base from the versioned KG state immediately before the **earliest grouped edit timestamp**, then apply all grouped edits in edit-timestamp order.

This shows the cumulative change introduced by the proposal group across time.

#### Mixed Active + Historical Groups

Mixed groups are out of scope for v1.

Combining active proposals with already-closed or executed proposals creates ambiguous base semantics: one side wants "live state now" while the other wants "state before the historical sequence began." Rather than inventing surprising behavior, v1 should reject mixed groups with a validation error.

### Response Shape

The grouped diff should preserve the existing entity diff payload and pagination model.

At minimum, the grouped response should include:

```json
{
  "proposalIds": ["..."],
  "spaceId": "...",
  "mode": "active" | "historical",
  "entities": [...],
  "pagination": {
    "cursor": "...",
    "hasMore": true,
    "totalEntities": 123
  }
}
```

The entity diff entries themselves should remain the same shape returned by current proposal diffs.

## Request-Time Squash Design

The recommended v1 design is request-time squashing.

Flow:

1. Load all grouped proposals and their `Publish` actions
2. Validate:
   - all proposal IDs exist
   - all belong to the requested `spaceId`
   - all are in a compatible mode (`active` or `historical`)
3. Resolve each proposal's edit timestamp
4. Sort proposals by edit timestamp
5. Fetch all edit blobs from `ipfs_cache`
6. Decode all edits
7. Concatenate ops into one ordered op stream
8. Extract affected entities once across the full grouped op list
9. Fetch the shared base state once
10. Apply all ops in order and diff once

### Why Start Here

- It reuses the existing in-memory proposal diff pipeline
- It keeps the first iteration focused on semantics instead of infrastructure
- The current implementation already batches base-state queries efficiently
- It gives us a clean way to validate grouped diff behavior before adding new persistence

## Indexed Proposal Ops (Future Optimization)

If grouped diff usage becomes heavy, the next step should be indexing normalized proposal ops into Postgres at ingest time.

The goal is not to precompute full diff payloads immediately. The better first optimization is to eliminate repeated:

- blob fetches from `ipfs_cache`
- edit decoding
- op-to-entity extraction

A future proposal-op index could store, per proposal and in edit order:

- proposal ID
- edit timestamp / edit ID
- op order
- normalized op payload
- directly affected entity IDs
- directly affected relation IDs

That would let the grouped diff handler query ordered ops and affected entities directly from Postgres, while still using the existing diff engine for base-state fetch and diff computation.

## Alternatives Considered

### 1) Accept Both Proposal IDs and Edit IDs in v1

Rejected for v1.

It widens the contract before we have proven the grouped semantics. Proposal IDs are the right public abstraction because grouped diffing is a governance feature, not just an edit-decoding feature.

### 2) Overload the Existing Singular Route

Rejected.

The singular route should remain simple and backward-compatible. Grouped diff deserves a separate additive contract.

### 3) Precompute Full Proposal Group Diffs

Rejected for now.

Full precomputed diff payloads create invalidation and storage complexity too early. The first real bottleneck is repeated proposal edit decode work, not the diff engine itself.

### 4) Support Mixed Active/Historical Groups in v1

Rejected.

The semantics are ambiguous and likely to surprise callers. We should add this only if a clear product use case emerges.

## Migration Plan

1. Add the grouped diff API contract
2. Implement request-time squashing on top of the existing proposal diff engine
3. Add validation for homogeneous group mode and single-space scope
4. Reuse current pagination and entity diff response shape
5. Measure latency and memory behavior with multi-proposal groups
6. Add indexed proposal ops later only if request-time squashing is too expensive

## Open Questions

- Should the grouped route return only `mode`, or also per-proposal metadata such as status and edit timestamp?
- Do we want to cap group size in v1 to protect handler latency?
- For the indexed-op path, is a normalized op table enough, or do we also want pre-indexed affected entity rows?
