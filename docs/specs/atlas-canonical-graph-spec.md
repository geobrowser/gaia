# Atlas Canonical Graph Specification

This document is the holistic, implementation-level specification for Atlas canonical graph computation and diff emission.

It consolidates and normalizes behavior from:
- RFC 0001 (canonical graph inputs)
- RFC 0002 (incremental diff emission)
- implementation decisions merged after multi-agent review

If there is any conflict between older design docs and this file, this file is authoritative.

## Goals

- Define which topology inputs can affect canonical graph state.
- Define canonical inclusion semantics independent of storage internals.
- Define deterministic diff output semantics for downstream replay.
- Define block-scoped emission behavior for stable consumer application.

## Non-Goals

- Prescribing internal data structures (maps/vectors/cache layouts).
- Prescribing a specific traversal implementation strategy.
- Defining downstream consumer storage/indexing architecture.
- Defining deployment topology or auth mode.

## Overview

Atlas consumes topology events, maintains graph state, computes canonical membership relative to a configured root, and emits canonical graph diffs to Kafka.

At a high level:
1. Ingest block-scoped topology actions.
2. Update graph state and transitive cache.
3. Recompute canonical graph when a block may affect canonical membership.
4. Emit one net diff per block.

## Normative Conventions

The terms MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are used with their RFC 2119 meanings.

This specification is primarily an input/output and behavior contract. Internal data structures and implementation techniques are non-normative unless explicitly called out.

## Data Model

### Identifiers

- `SpaceId`: 16-byte identifier.
- `TopicId`: 16-byte identifier.

### Edge Types

- `Root`
- `Verified`
- `Related`
- `Editor`
- `Member`
- `Topic { topic_id }`

Canonical-granting edges:
- `Verified`, `Related`, `Editor`, `Member`

Non-canonical-granting edge:
- `Topic`

## Inputs

### Canonical Root

- Input: configured root `SpaceId`.
- Rule: root is always canonical.

### Topology Event Inputs

| Input Event | Effective Mutation | Can Expand Canonical Set? |
| --- | --- | --- |
| `SpaceCreated` | Add/update space + topic membership metadata | No |
| `Verified` / `VerifiedRemoved` | Add/remove explicit Verified edge | Yes (add) / Can shrink (remove) |
| `Related` / `RelatedRemoved` | Add/remove explicit Related edge | Yes (add) / Can shrink (remove) |
| `EditorAdded` / `EditorRemoved` | Add/remove explicit Editor edge | Yes (add) / Can shrink (remove) |
| `MemberAdded` / `MemberRemoved` | Add/remove explicit Member edge | Yes (add) / Can shrink (remove) |
| `Subtopic` / `SubtopicRemoved` | Add/remove topic edge | No |

Notes:
- Topic membership updates can change attachment/positioning, but cannot introduce new canonical nodes.
- Event ordering is the input order within each block.

### Persistent Graph State

Implementations need a persistent graph state that can represent:
- explicit edges (`source -> [(target, explicit_edge_type)]`)
- topic edges (`source -> {topic_id}`)
- topic membership/indexes (`topic -> {spaces}` and reverse indexes)
- space-to-topic mapping for `SpaceCreated`

Observable requirement: `SpaceCreated` re-announcements must not leave stale topic membership behavior in outputs.

## Input Event Semantics

Supported topology mutations include:
- `SpaceCreated`
- `Verified` / `VerifiedRemoved`
- `Related` / `RelatedRemoved`
- `Subtopic` / `SubtopicRemoved`
- `EditorAdded` / `EditorRemoved`
- `MemberAdded` / `MemberRemoved`

Semantics:
- `SpaceCreated` updates known-space and topic membership indices but does not itself grant canonical membership.
- explicit edge additions/removals can expand/shrink canonical membership.
- topic edge additions/removals only affect attachment/positioning of already-canonical subtrees.

## Canonical Membership Semantics

A space is canonical iff:
1. it is the configured root, or
2. it is reachable from root via one or more canonical-granting explicit edges.

### Semantic Invariants

1. Topic edges MUST NOT grant canonical membership.
2. Root MUST be present in canonical membership.
3. Canonical tree root `space_id` MUST equal configured root.
4. Canonical tree MAY contain duplicate `SpaceId` nodes via different attachment paths.
   - This is intentional.
   - Duplicates do not imply duplicate canonical membership in the flat set.

## Computation Model (Behavioral)

Canonical computation is two-phase:

### Phase 1: Explicit Reachability

- Perform BFS from root over canonical-granting explicit edges.
- Build canonical membership set and explicit tree structure.
- Use visited tracking to prevent cycles and repeated explicit inclusion.

### Phase 2: Topic Subtree Attachment

- For canonical source nodes with topic edges, resolve topic members.
- Attach filtered transitive subtrees for already-canonical members.
- This phase MUST preserve the rule that topic edges do not expand canonical set.

## Block Processing and Emission Semantics

Processing is block-scoped.

Within a block, for each event in order:
1. Evaluate `affects_canonical` against pre-mutation canonical context.
2. Call transitive cache handler on pre-mutation state.
3. Apply event mutation to graph state.

After all events in the block:
- If no event may affect canonical state, skip recomputation and emission.
- Otherwise, compute canonical graph once.
- Compute diff against last emitted state.
- Emit at most one diff message for the block.

This block-level batching behavior is normative. Atlas MUST NOT emit intermediate per-event diffs within a single block.

## Diff Semantics

Diff changes are emitted as:
- `ADDED`
- `REMOVED`
- `MOVED`

Rules:
- `ADDED` and `MOVED` include position (`distance`, `parent_edge`).
- `REMOVED` omits position.
- Diff ordering MUST be deterministic (sorted by `SpaceId`).
- For duplicate tree occurrences of a `SpaceId`, diff tracking MUST retain one canonical position at shortest distance from root.

## Determinism Requirements

Atlas output MUST be deterministic for identical input streams and root configuration.

Determinism requirements:
- stable traversal/order semantics
- stable diff ordering by `SpaceId`
- stable topic prefix resolution from `ENVIRONMENT`

## Kafka Emission Contract

Message type:
- `CanonicalGraphDiff`

Topic resolution:
- Base topic: `KAFKA_TOPIC` (default `topology.canonical` in runtime)
- Prefix by `ENVIRONMENT`:
  - `staging` -> `staging.`
  - `production` -> no prefix

Producers MAY use plaintext in local/dev and SASL/SSL when credentials are present.

## Outputs

### Wire Message Format

Canonical output wire format is defined by `hermes-schema/proto/topology.proto`.

#### CanonicalGraphDiff

- `root_id: bytes` - root space this diff applies to
- `changes: repeated NodeChange` - batch of node changes
- `meta: blockchain_metadata.BlockchainMetadata` - block/cursor metadata

#### NodeChange

- `space_id: bytes` - changed space
- `change_type: ChangeType` - `ADDED | REMOVED | MOVED`
- `distance: optional uint32`
  - present for `ADDED` and `MOVED`
  - absent for `REMOVED`
- `parent_edge: optional EdgeInfo`
  - present for `ADDED` and `MOVED`
  - absent for `REMOVED`

#### EdgeInfo

- `parent_id: bytes` - parent space id
- `edge_type: oneof`
  - `verified`
  - `related`
  - `topic` (includes `topic_id`)
  - `editor`
  - `member`

#### ChangeType enum mapping

- `CHANGE_TYPE_ADDED`
- `CHANGE_TYPE_REMOVED`
- `CHANGE_TYPE_MOVED`

(`CHANGE_TYPE_UNSPECIFIED` exists in schema as proto default and should not be emitted by Atlas.)

### Canonical Graph Diff Output

Output message:
- `CanonicalGraphDiff`

Per-change output:
- `ADDED`: node entered canonical set; includes position (`distance`, `parent_edge`)
- `REMOVED`: node left canonical set; omits position
- `MOVED`: node remains canonical but position changed; includes position

Output guarantees:
- changes in a diff are sorted by `SpaceId` for deterministic application
- each emitted diff is a complete batch for one block-level update
- empty diffs are not emitted
- root node is implicit and not emitted as a change

### Consumer Replay Model

Consumers should apply each diff atomically and in order. Replaying the ordered diff stream from the same root and topic yields the same canonical tree shape and node positions.

## Implementation Invariants and Decisions (Non-Normative)

These are current implementation constraints and engineering decisions, captured for maintainers. They are not part of the external protocol contract.

- Keep critical postconditions enforced in release builds to prevent corrupt emissions.
- Prefer iterative traversal in deep-tree paths to avoid stack-overflow risk.
- Ensure protobuf edge mapping failures are observable and fail safely.
- Keep stale forward/reverse topic mappings from leaking into effective behavior.

## Performance Characteristics (Target)

- Reachability and attachment should scale approximately O(V + E) per affecting recomputation.
- Diff computation should use allocation-conscious structures (sorted vectors + buffer reuse).
- Emission should avoid redundant no-op messages (empty diffs are not emitted).

## Conformance Test Matrix

An implementation is conformant only if tests cover at least:
- canonical membership via Verified/Related/Editor/Member
- topic edge non-granting behavior
- edge removals shrinking canonical set
- cycles and deterministic output
- duplicate `SpaceId` in tree with shortest-distance diff behavior
- block-level batching (single net diff per block)
- protobuf encode/decode correctness for edge types and node changes
- runtime with real Kafka producer/consumer path

## Non-Goals

- This spec does not define downstream consumer storage/indexing strategy.
- This spec does not define versioning strategy for non-topology topics.
- This spec does not mandate a specific deployment topology.

## References

- `docs/specs/canonical-graph.md`
- `docs/specs/versioned-diffing.md`
- `hermes-schema/proto/topology.proto`
- `atlas/src/main.rs`
- `atlas/src/graph/canonical.rs`
- `atlas/src/graph/state.rs`
- `atlas/src/graph/diff.rs`
- `atlas/src/graph/transitive.rs`
- `atlas/src/kafka/emitter.rs`
