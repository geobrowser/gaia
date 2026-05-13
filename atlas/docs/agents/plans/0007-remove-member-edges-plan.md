# 0007: Remove Member and Editor Edges from Atlas

Remove member-of-space and editor-of-space relationships from Atlas's traversal and canonical graphs. Both edge types should have zero effect on any computed output.

After this plan lands, the only canonical-granting edge types in Atlas are **Verified** and **Related**. **Subtopic** edges appear in the canonical graph too, but they do not grant canonical membership: Phase 1 of canonical computation derives the canonical set from explicit (Verified/Related) edges only, and Phase 2 then attaches Subtopic edges filtered to nodes that are already canonical. See [`atlas/src/graph/canonical.rs`](../../../src/graph/canonical.rs) for the two-phase implementation.

Tracks Linear issue [GEO-618](https://linear.app/defi-wonderland/issue/GEO-618/remove-member-relationships-from-canonical-graph).

> **Status (updated 2026-05-13)**
> - PR [#194](https://github.com/defi-wonderland/gaia/pull/194) is **merged** (2026-05-12) into the integration branch `geo-618-remove-member-edges`. Per the resolved decision below (option A), it drops **both Member and Editor** edges in a single change and bumps the checkpoint marker `atlas-v1 → atlas-v2`. Closes [GEO-624](https://linear.app/defi-wonderland/issue/GEO-624).
> - PR [#197](https://github.com/defi-wonderland/gaia/pull/197) is the docs follow-up — it updates `atlas/docs/graph-concepts.md` and this plan to reflect the post-merge state. Tracks [GEO-625](https://linear.app/defi-wonderland/issue/GEO-625).
> - Operational rollout (fresh bootstrap to `atlas-v2` on staging and prod) is tracked by [GEO-626](https://linear.app/defi-wonderland/issue/GEO-626).

## Background

Atlas previously treated `MEMBER_ADDED` / `MEMBER_REMOVED` and `EDITOR_ADDED` / `EDITOR_REMOVED` chain actions as topology edges that granted canonical membership (historical behavior, pre-0007). The conversion paths were structurally identical:

```
// Historical behavior (pre-0007):
chain Action (MEMBER_ADDED or EDITOR_ADDED)
  → convert.rs::convert_member_added | convert_editor_added
  → TrustExtension::MemberAdded | TrustExtension::EditorAdded
  → GraphState::apply_trust_extended
  → explicit_edges[source].push((target, EdgeType::Member | EdgeType::Editor))
```

Once stored as explicit edges, Member and Editor edges were indistinguishable from `Verified` / `Related` in both the transitive BFS (`transitive.rs::compute`) and the canonical Phase-1 BFS (`canonical.rs::compute_if_changed`). They expanded the canonical set and the transitive reachable set.

This was introduced by plan [0006: Live Substream, Membership, and Graph Diffing](./0006-live-substream-membership-diffing-plan.md). Plan 0007 reverses the membership portion of that plan in full.

## Goal

After this change:
- `MemberAdded` / `MemberRemoved` / `EditorAdded` / `EditorRemoved` events applied to any `GraphState` produce no change to `explicit_edges`, no change to the transitive graph rooted at any space, and no change to the canonical graph.
- `topology.canonical` Kafka messages never contain `MemberEdge` or `EditorEdge` entries.
- `EdgeType::Member` AND `EdgeType::Editor` cease to exist in the Atlas code.
- The chain actions `MEMBER_ADDED` / `MEMBER_REMOVED` / `EDITOR_ADDED` / `EDITOR_REMOVED` are still parsed (chain compatibility) — they just don't mutate graph state.

## Approach

Two viable cuts:

| Approach | Pros | Cons |
|----------|------|------|
| **A. Drop at storage** (chosen) | One change point per edge type. BFS code stays uniform. Impossible to forget a filter site. | Loses ability to expose Member/Editor as non-canonical relations later without re-introducing storage. |
| **B. Filter at traversal** | Member/Editor still tracked in state — can be exposed elsewhere. | Two filter sites (transitive + canonical Phase 1) per edge type, must stay in sync. Easy to regress. |

We pick **A**: stop applying Member and Editor events to `GraphState`, then delete the `EdgeType::Member` and `EdgeType::Editor` variants. The compiler will surface every dead match arm.

## Changes

### 1. No-op Member and Editor events in GraphState

**File:** `atlas/src/graph/state.rs`

Remove the `MemberAdded` / `MemberRemoved` and `EditorAdded` / `EditorRemoved` arms from `apply_trust_extended`. Replace with an explicit no-op arm so the next reader knows it's intentional:

```rust
// Member and Editor edges are ignored — see plan 0007. Kept in the enum
// for chain compatibility (convert.rs still emits them).
TrustExtension::MemberAdded { .. }
| TrustExtension::MemberRemoved { .. }
| TrustExtension::EditorAdded { .. }
| TrustExtension::EditorRemoved { .. } => {}
```

No changes needed to `events.rs` — `TrustExtension::MemberAdded` / `MemberRemoved` / `EditorAdded` / `EditorRemoved` variants stay so `convert.rs` keeps compiling. They become payloads that pass through without effect.

### 2. Remove EdgeType::Member and EdgeType::Editor

**File:** `atlas/src/graph/tree.rs`

Delete the `Member` and `Editor` variants from `EdgeType`. The compiler will fail every match site. Fix each:

- **`atlas/src/kafka/emitter.rs`** — drop the `EdgeType::Member => ...` and `EdgeType::Editor => ...` arms in both `edge_to_proto` functions. The `MemberEdge` / `EditorEdge` proto types stay defined (downstream consumers may still reference them) but Atlas never emits them.
- **`atlas/src/kafka/emitter.rs`** (test fixture) — drop the `Member` and `Editor` test fixture entries from `test_tree_node_to_proto_all_edge_types`, or substitute with `Verified` / `Related` if the test still needs multiple children.
- **`atlas/src/persistence.rs`** — see persistence section below.
- **`atlas/benches/graph_diff.rs`** — remove `EdgeType::Member` and `EdgeType::Editor` from both benchmark fixture arrays.

### 3. Transitive cache invalidation

**File:** `atlas/src/graph/transitive.rs`

`handle_event` invalidates `member_space_id` for Member/Editor events. With both now no-ops, invalidation is unnecessary. Move both into the same no-op arm:

```rust
// Member and Editor edges: no-op — these events do not mutate state (plan 0007).
TrustExtension::MemberAdded { .. }
| TrustExtension::MemberRemoved { .. }
| TrustExtension::EditorAdded { .. }
| TrustExtension::EditorRemoved { .. } => {}
```

### 4. Persistence — hard cut via marker bump

**Files:** `atlas/src/persistence.rs`

Hard-cut the checkpoint format. No Member or Editor data persists past this change. Three coordinated edits:

1. **Delete `PersistedEdgeType::Member` and `PersistedEdgeType::Editor`** and their match arms in `From<EdgeType>` / `TryFrom<PersistedEdgeType>` / the ordering helper. After this, the type system guarantees no Member or Editor edge can be serialized or deserialized.

2. **Bump the default `runtime_compatibility_marker`** to `atlas-v2`. Atlas already validates this marker on load — any pre-existing checkpoint will be rejected as incompatible without us needing to rely on enum-deserialize failures.

3. **Update test fixtures** that hardcode the marker string.

**Marker version coordination**: the marker is `atlas-v2` and covers both the Member and Editor removals in a single fresh bootstrap (see [Resolved decisions](#resolved-decisions)).

Combined with `ATLAS_CHECKPOINT_ALLOW_FRESH_START=true` during deploy (see Rollout below), Atlas detects the marker mismatch, logs `Checkpoint rejected; … starting fresh`, and rebuilds graph state from `SUBSTREAMS_START_BLOCK`. The first new checkpoint stamps as the new marker.

No manual checkpoint deletion is required. No backwards-compat shim is introduced.

### 5. Convert layer stays as-is

**File:** `atlas/src/convert.rs`

No changes. `convert_member_added` / `convert_member_removed` / `convert_editor_added` / `convert_editor_removed` keep producing their respective `TrustExtension` variants from chain actions. These flow into `GraphState::apply_trust_extended` and hit the no-op arm.

Rationale: the conversion layer mirrors the chain ABI. Keeping it stable means we can re-enable Member or Editor edges later without re-plumbing the parse path.

### 6. Main loop logging

**File:** `atlas/src/main.rs`

The diagnostic logging for `MemberAdded` / `MemberRemoved` / `EditorAdded` / `EditorRemoved` stays — useful for debugging "why didn't this space appear?" questions. Optionally annotate the log message with `(ignored)`.

## Tests

### Existing tests to update

**`atlas/tests/e2e.rs`** — `.member_added(...)` / `.member_removed(...)` / `.editor(...)` / `.editor_removed(...)` helpers still feed events into the pipeline. Update any test that asserts canonical or transitive membership through a Member or Editor edge:

- Cases where Member/Editor is the only path to a node: assert that node is NOT in canonical and NOT in the transitive set.
- Cases where Member/Editor is one of multiple paths: assert the node is still present via the non-Member, non-Editor path.

Specific tests known to need updates (relative to current state — some Member tests may already be flipped by PR #194):
- Tests using `.editor(...)` to grant canonical membership.
- Tests asserting `editor_removed` produces a `Removed` change.
- Tests asserting transitive reachability through an Editor edge.

### New tests to add (symmetric to the Member tests added in PR #194)

In `atlas/src/graph/state.rs` tests:

- `test_editor_added_is_no_op`: apply `EditorAdded(source, target)`, assert `state.explicit_edges` is empty.
- `test_editor_removed_is_no_op`: same shape.

In `atlas/src/graph/canonical.rs` tests:

- `test_editor_edge_from_root_does_not_grant_canonical`: root has `EditorAdded(root, X)`, assert canonical set is `{root}` only.

In `atlas/src/graph/transitive.rs` tests:

- `test_editor_edge_not_in_transitive`: apply `EditorAdded(A, B)`, compute `get_full(A)`, assert B is not reachable.

### Benches

`atlas/benches/graph_diff.rs` — drop `EdgeType::Member` and `EdgeType::Editor` entries from both fixture arrays. If the bench depends on edge-type variety, the remaining `Verified` / `Related` types suffice.

## Docs

- `atlas/docs/graph-concepts.md` — drop "Member relationships" AND "Editor relationships" from the Explicit Edges list. Note Member and Editor events exist on-chain but are not represented in any Atlas graph.
- `atlas/docs/algorithm-overview.md` — no changes needed (algorithm description already only references explicit/topic categories).
- `atlas/README.md` — already lists only Verified/Related, no change.
- Wiki: `~/knowledge-base/projects/geo/concepts/atlas-canonical-graph.md` — drop Member AND Editor from "canonical-granting edges" line. Add a note that Member and Editor events are no-oped.

## Wire-format and downstream impact

`topology.canonical` is a derived snapshot stream. After this change:

- The next emitted `CanonicalGraphDiff` will contain `REMOVED` entries for every space that was previously canonical solely via a Member or Editor edge.
- Downstream consumers (kg-indexer, search-indexer) apply diffs as authoritative — they will correctly drop those spaces from their projections.
- Per `atlas/docs/known-issues.md`, downstream treats canonical updates as full state snapshots; idempotent application means no special handover is needed beyond the normal diff.

No new "reset" signal required. No new topic version required.

**Impact estimate**: Editor edges are likely more common than Member edges in the existing graph (every DAO has at least one editor, but not every DAO has member-only relationships). The `REMOVED` diff after this lands will be correspondingly larger if Editor and Member ship together.

## Rollout

The marker bump to `atlas-v2` makes every existing checkpoint incompatible by construction. Atlas will refuse to start unless we permit a fresh bootstrap.

1. Land code change + tests + docs.
2. On every environment running Atlas (staging, prod, anywhere with `ATLAS_CHECKPOINT_DATABASE_URL` set): set `ATLAS_CHECKPOINT_ALLOW_FRESH_START=true` **before** deploying the new image.
3. Deploy. Atlas logs `Checkpoint rejected; ATLAS_CHECKPOINT_ALLOW_FRESH_START enabled, starting fresh` and replays from `SUBSTREAMS_START_BLOCK`.
4. Watch during replay:
   - Atlas `latest_block` metric catches up to chain head.
   - `topology.canonical` consumer-group lag spikes as kg-indexer / search-indexer process the rebuild stream — should settle once replay completes. Existing diff-apply logic is idempotent so no special handling required.
   - First new checkpoint row written with `runtime_compatibility_marker = atlas-v2`.
5. Spot-check known Member-only and Editor-only canonical spaces: confirm they're gone from canonical queries.
6. After Atlas is stable on the new marker, **revert `ATLAS_CHECKPOINT_ALLOW_FRESH_START` to `false`** so future checkpoint corruption fails loud instead of silently bootstrapping from genesis.

**Replay-cost caveat:** default `SUBSTREAMS_START_BLOCK=82655`. Confirm acceptable replay wall-time per environment before scheduling the deploy.

## Resolved decisions

### 1. Sequencing of Editor relative to PR #194

**Outcome: option A was chosen.** PR [#194](https://github.com/defi-wonderland/gaia/pull/194) was extended on the `geo-618-remove-member-edges` branch to drop **both** Member and Editor edges in a single change, and the checkpoint marker was bumped once (`atlas-v1 → atlas-v2`) to cover both removals via a single fresh bootstrap.

## File checklist

Reflects total scope (Member + Editor). Items already shipped by PR #194 are marked as such.

| File | Member change | Editor change |
|------|---------------|---------------|
| `atlas/src/graph/state.rs` | No-op (✅ #194) | No-op (✅ #194) |
| `atlas/src/graph/tree.rs` | Remove `EdgeType::Member` (✅ #194) | Remove `EdgeType::Editor` (✅ #194) |
| `atlas/src/graph/transitive.rs` | Move to no-op arm (✅ #194) | Move to no-op arm (✅ #194) |
| `atlas/src/kafka/emitter.rs` | Drop match arms + fixture (✅ #194) | Drop match arms + fixture (✅ #194) |
| `atlas/src/persistence.rs` | Drop `PersistedEdgeType::Member` + bump marker (✅ #194 → `atlas-v2`) | Drop `PersistedEdgeType::Editor` (✅ #194, single marker bump to `atlas-v2` covers both) |
| `atlas/src/main.rs` | Optional log annotation | Optional log annotation |
| `atlas/tests/e2e.rs` | Flip Member expectations (✅ #194) | Flip Editor expectations (✅ #194) |
| `atlas/benches/graph_diff.rs` | Drop `Member` fixture (✅ #194) | Drop `Editor` fixture (✅ #194) |
| `atlas/src/graph/state.rs` (tests) | `test_member_*_is_no_op` (✅ #194) | `test_editor_*_is_no_op` (✅ #194) |
| `atlas/src/graph/canonical.rs` (tests) | Member canonical assertion (✅ #194) | Editor canonical assertion (✅ #194) |
| `atlas/src/graph/transitive.rs` (tests) | Member transitive assertion (✅ #194) | Editor transitive assertion (✅ #194) |
| `atlas/docs/graph-concepts.md` | Drop Member from Explicit Edges (✅ PR #197 — GEO-625) | Drop Editor from Explicit Edges (✅ PR #197 — GEO-625) |
| `~/knowledge-base/projects/geo/concepts/atlas-canonical-graph.md` | Drop Member from canonical-granting edges (✅ wiki) | Drop Editor from canonical-granting edges (✅ wiki) |

## Related

- [0006: Live Substream, Membership, and Graph Diffing](./0006-live-substream-membership-diffing-plan.md) — introduced Member and Editor edges; this plan reverses the membership portion in full.
- [Graph Concepts](../../graph-concepts.md)
- [Known Issues](../../known-issues.md)
- PR [#194](https://github.com/defi-wonderland/gaia/pull/194) — drops both Member and Editor edges, bumps checkpoint marker to `atlas-v2`.
- PR [#197](https://github.com/defi-wonderland/gaia/pull/197) — this docs PR; updates `atlas/docs/graph-concepts.md` and commits plan 0007.
