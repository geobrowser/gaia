---
title: "feat: Add space topic proposal end-to-end coverage"
type: feat
date: 2026-03-19
---

# feat: Add space topic proposal end-to-end coverage

## Overview

Add end-to-end coverage for proposal-driven space topic changes so both proposal ingestion and proposal read APIs support setting and unsetting a space topic.

Today the codebase supports:

- direct `TOPIC_DECLARED` events in the topic pipeline and topic consumers
- subspace topic proposal actions decoded from `ping(...)`
- proposal read APIs for the existing action types

But it does not yet cover space-level topic proposal actions. The current proposal decoder only recognizes subspace ping variants, and the existing e2e fixture set only exercises the original seven proposal actions plus six subspace proposal actions.

## Problem Statement / Motivation

We need confidence in two user-facing flows:

1. **Write flow:** a proposal containing a space-topic action is decoded, stored, and indexed with the right action type and target topic semantics.
2. **Read flow:** proposal APIs expose those actions with stable typed responses and filters, and executed topic changes are reflected in the space topic state if the e2e harness already includes execution/topic events.

Current gaps found in local research:

- `hermes-pipeline/src/pipelines/governance.rs:258` only decodes `Ping` proposal actions through `decode_ping_subspace_action`, which recognizes subspace actions only.
- `api/src/proposals/types.ts:17` defines no space-topic proposal action types.
- `kg-indexer/tests/e2e.rs:126` and `kg-indexer/tests/e2e.rs:527` cover 13 proposal fixtures total, with no space-topic proposal cases.
- `kg-indexer/src/handlers/topics.rs:13` and `kg-indexer/src/storage.rs:458` model topic updates as set-only, not explicit clear/unset.
- `search-indexer/src/consumer/space_topics_consumer.rs:337` also treats topic events as declaration-only updates.

Protocol context is now clear on both points:

- spaces set topics onchain via proposal actions that call `ping(...)`
- unset is a distinct action, `TOPIC_REMOVED`, not a nil/zero `TOPIC_DECLARED`

For proposal actions, the wire model is:

- `SET_TOPIC` proposal intent maps to `ping(ActionsConstants.TOPIC_DECLARED, bytes32(topicId), data)`
- `UNSET_TOPIC` proposal intent maps to `ping(ActionsConstants.TOPIC_REMOVED, bytes32(topicId), data)`
- the topic ID is packed as `bytes32(bytes16(topicId))`, so the target topic lives in `topic[0..16]` and the remaining 16 bytes are zero padding

## Proposed Solution

Keep the scope centered on proposal end-to-end coverage, with one narrow extension into executed topic state where it is already part of the same fixture path.

### Architecture decision

Represent the **proposal action** using proposal-intent names, not event names:

- `SetTopic`
- `UnsetTopic`

Reasoning:

- proposals describe the requested action, while `TOPIC_DECLARED` is the execution-time event
- this avoids overloading proposal APIs with event terminology
- it keeps the proposal layer parallel with `AddMember`, `RemoveEditor`, and `UpdateVotingSettings`

If implementation reveals the contract surface already has stronger canonical names, follow the contract naming instead. The key is to use one naming scheme consistently across proto, indexer storage, and API responses.

### Scope

In scope:

- proposal-action decoding for space-topic set/unset
- proposal storage/indexing
- proposal API response typing and filtering
- e2e fixtures and assertions for both set and unset

Out of scope:

- executed-state assertions on `spaces.topic_id`
- broader search-index ranking changes
- new search endpoints
- unrelated refactors in existing topic consumers

## Technical Approach

### Phase 1: Confirm protocol encoding before writing fixtures

Document the confirmed proposal calldata and execution event shape:

- set topic: `ping(TOPIC_DECLARED, bytes32(topicId), data)`
- unset topic: `ping(TOPIC_REMOVED, bytes32(topicId), data)`

Implementation note:

- unlike subspace topic proposal actions, the target topic is packed in `topic[0..16]`, not `topic[16..32]`
- this is the same `bytes32(bytes16(...))` layout used by direct topic events

### Phase 2: Extend proposal write-path coverage

#### `hermes-relay/src/source/mock_events.rs`

Add proposal action builders for space-topic proposal actions alongside the existing helper set at `hermes-relay/src/source/mock_events.rs:329`.

Likely additions:

- `ProposalAction::set_topic(...)`
- `ProposalAction::unset_topic(...)`

Update the mock topology/proposal fixture sequence near `hermes-relay/src/source/mock_events.rs:1216` to add two new proposals:

- one set-topic proposal
- one unset-topic proposal

Keep them explicit and adjacent so the e2e tests can reason about them without hidden coupling.

#### `hermes-schema` + `hermes-pipeline`

Extend governance proposal action decoding so space-topic proposal actions are first-class, similar to the recent subspace work:

- add proto enum/oneof support for the new proposal action variants
- extend `decode_proposal_action` in `hermes-pipeline/src/pipelines/governance.rs:258`
- generalize `decode_ping_subspace_action` into a broader ping decoder, or add a sibling branch that handles `TOPIC_DECLARED` / `TOPIC_REMOVED`-based proposal actions

Important detail:

- the proposal decoder should keep selector classification separate from ping payload interpretation, following the existing pattern
- do not special-case space-topic proposals in `main.rs`; the work belongs in governance decode, not top-level routing
- do not reuse the subspace topic byte layout; space topic actions should decode the target from `topic[0..16]`

#### `kg-indexer`

Extend governance mapping and storage so the new proposal actions land in `proposal_actions` with stable action types and target semantics.

Expected storage semantics:

- set topic: `target_id` = topic entity UUID
- unset topic: `target_id = NULL`

Even though `TOPIC_REMOVED` may still carry the removed topic ID in calldata/event topic, prefer `NULL` in `proposal_actions` for the proposal intent. That keeps proposal read semantics clean: the action is “unset topic”, not “set to this topic”.

#### Write-path e2e assertions

Update `kg-indexer/tests/e2e.rs`:

- increase the expected proposal count from 13 to 15
- extend `expected::proposal_action_types()` to include the two new proposal IDs
- assert the stored `action_type` and `target_id` behavior for set and unset

If unset uses `NULL` storage semantics, add an explicit assertion for `target_id IS NULL` rather than relying on omission.

### Phase 3: Extend proposal read-path coverage

#### `api/src/proposals/types.ts`

Add the new action types to `PROPOSAL_ACTION_TYPES` and define typed response shapes next to the existing subspace action models at `api/src/proposals/types.ts:232`.

#### `api/src/proposals/router.ts`

Extend `mapToActionResponse` to return typed responses for the new actions, following the same discriminated-union pattern used for every other proposal action.

#### `api/src/proposals/__tests__/queries.test.ts`

Add coverage for:

- list/detail responses including `SET_TOPIC` and `UNSET_TOPIC` style actions
- filtering by the new `actionTypes` values
- unset responses using the chosen external shape

Recommended response shape:

- set topic: `{ actionType: "SET_TOPIC", targetTopicId: "<uuid>" }`
- unset topic: `{ actionType: "UNSET_TOPIC" }`

That keeps the API explicit and avoids leaking execution transport details into the proposal response.

### Phase 4: Cover executed topic state only where it is already on the path

The direct topic pipeline already decodes `TOPIC_DECLARED` into `HermesTopicDeclared` in `hermes-pipeline/src/pipelines/topics.rs:51`, and the KG/search consumers currently treat it as declaration-only.

Add one narrow verification path for execution outcomes:

- after the set-topic proposal executes, `spaces.topic_id` should equal the target topic
- after the unset proposal executes, `spaces.topic_id` should be cleared to `NULL`

This likely requires one small topic-state change:

- `kg-indexer/src/handlers/topics.rs:13` and `kg-indexer/src/storage.rs:458` need a clear/unset representation if executed `TOPIC_REMOVED` events are consumed through the same path as `TOPIC_DECLARED`

Because search-indexer clearing is out of scope for this task, stop at proposal coverage unless the existing execution fixture already passes through this path naturally.

Only extend `search-indexer` tests if the existing proposal e2e harness already observes those topic messages. Otherwise, stop at KG/API coverage and create a follow-up task for search-indexer clearing semantics.

## Spec Flow Notes

Primary user flows:

1. Editor creates a proposal to set the space topic.
2. The governance pipeline decodes the action and stores it as a typed proposal action.
3. Proposal APIs return the action in list/detail responses.
4. Proposal executes and the space topic becomes visible in persisted read state.
5. Editor creates a proposal to unset the space topic.
6. The same write/read path works, and execution clears the space topic cleanly.

Edge cases to cover:

- unset when a topic is already present
- unset when topic is already absent
- set then unset in the same fixture sequence
- `TOPIC_DECLARED` and `TOPIC_REMOVED` proposal pings decode the target topic from the correct byte slice
- replay/idempotency for repeated topic events

## Acceptance Criteria

- [x] The proposal fixture set includes one space-topic set proposal and one space-topic unset proposal.
- [x] Governance decoding classifies those proposal actions as first-class typed actions instead of `Unknown`.
- [x] `proposal_actions` rows are written for both new proposals with documented target semantics.
- [x] Proposal list/detail APIs return typed action payloads for both actions.
- [x] `actionTypes` filtering supports the new proposal action types.
- [x] End-to-end tests cover both the write path and the proposal read path.

## Success Metrics

- Proposal ingestion e2e suite proves 15 proposal fixtures are indexed with no `Unknown` fallback for space-topic proposal actions.
- Proposal API tests demonstrate stable response typing for set/unset.
- No proposal consumer falls back to `Unknown` for `TOPIC_DECLARED` / `TOPIC_REMOVED` proposal pings.

## Dependencies & Risks

### Main risks

- **Wrong byte-slice decode**: The most likely bug is reusing subspace topic decoding (`topic[16..32]`) for space-topic proposals, when the direct topic layout uses `topic[0..16]`.
- **Proposal/event semantic mismatch**: The wire actions are `TOPIC_DECLARED` / `TOPIC_REMOVED`, but the proposal API should expose `SET_TOPIC` / `UNSET_TOPIC`. Mixing those layers would make filtering and client logic inconsistent.
- **Scope creep into general topic consumers**: Search-indexer clearing behavior may need a separate follow-up if proposal e2e coverage does not already exercise that path.

### Mitigations

- add explicit tests for both byte layouts: direct space-topic and subspace-topic
- normalize proposal actions into proposal-intent names at the API boundary
- keep the first implementation focused on proposal metadata + KG state, only extending search-indexer if the existing e2e path naturally reaches it

## References & Research

Internal references:

- `docs/protocol/knowledge-graph-ontology.md:257` documents that spaces set topics onchain via `SET_TOPIC`
- contract implementation confirms proposal actions call `ping(...)` directly, with `TOPIC_DECLARED` and `TOPIC_REMOVED` action constants
- `hermes-pipeline/src/pipelines/governance.rs:258` shows proposal action decoding currently stops at subspace ping variants
- `api/src/proposals/types.ts:17` shows the proposal API action enum currently has no space-topic proposal types
- `hermes-pipeline/src/pipelines/topics.rs:51` shows direct topic events are already decoded through `TOPIC_DECLARED`
- `kg-indexer/src/handlers/topics.rs:13` shows topic events are modeled as set-only assignments
- `kg-indexer/src/storage.rs:458` updates `spaces.topic_id` as a non-null set operation
- `search-indexer/src/consumer/space_topics_consumer.rs:337` shows read-side topic consumers are declaration-only today
- `kg-indexer/tests/e2e.rs:126` and `kg-indexer/tests/e2e.rs:527` show current proposal e2e coverage stops at 13 proposals and subspace-topic proposal actions
- `hermes-relay/src/source/mock_events.rs:329` and `hermes-relay/src/source/mock_events.rs:1216` show the current proposal action builder surface and mock proposal sequence
- `docs/plans/2026-03-04-feat-decode-subspace-proposal-actions-plan.md` is the closest recent precedent for extending proposal action decoding through `ping(...)`

Research notes:

- No relevant brainstorm was found in `docs/brainstorms/`.
- No relevant institutional learning was found in `docs/solutions/` for space-topic proposal set/unset semantics.
- External research was skipped because this is an internal protocol-and-pipeline consistency task; the useful evidence is in the local protocol docs and existing decode/indexing code.
