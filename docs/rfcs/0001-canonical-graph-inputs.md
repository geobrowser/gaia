# Canonical Graph Inputs (RFC)

## Summary
Define which data inputs (events and derived edges) should include a space in the canonical graph, and which inputs should only add auxiliary connections. This RFC specifies the intended sources of explicit edges, topic edges, and topic membership that feed canonical computation.

## Date
2026-01-21

## Goals
- Define the event types that mutate the inputs to canonical graph computation.
- Make explicit which inputs can expand the canonical set vs. only add edges between canonical nodes.
- Align the Rust pipeline with the behavior in `@atlas` (mock stream + storage semantics).

## Non-Goals
- Changing the canonical graph algorithm.
- Defining how changes to the canonical graph are emitted.
- Describing Kafka schema details or storage/layout specifics.

## Proposal Context
This RFC is intentionally forward-looking: it specifies the desired canonical inputs and semantics as the target state for implementation and alignment.
The existing Atlas implementation is a proof of concept and is not actively used in the protocol at the time of this RFC.

## Definitions
- **Canonical set**: The set of spaces reachable from the canonical root via explicit edges only.
- **Explicit edges**: Edges that can grant canonical inclusion.
- **Topic edges**: Edges from a space to a topic. These only connect already-canonical nodes.
- **Topic membership**: The mapping from topic -> spaces that set that topic.

## Inputs

### 1) Canonical Root
- A single configured root space ID seeds canonical traversal.
- The root is always canonical and is included even if it has no edges.

### 2) Space Lifecycle (Spaces + Topic Setting)
**Source events**:
- `SpaceCreated` (from `SPACE_REGISTERED` in substreams)
- `TOPIC_SET` / `TOPIC_REMOVED` (from `@atlas` mock stream)

**Graph inputs**:
- Adds the space to the graph.
- Adds the space to topic membership for its set topic(s).

**Canonical impact**:
- Does **not** make the space canonical on its own.
- Only affects canonical output when a canonical node has a topic edge to that topic.

### 3) Explicit Edges (Canonical-Granting)
**Source events**:
- `TrustExtended::Verified` (from `SUBSPACE_VERIFIED`)
- `TrustExtended::Related` (from `SUBSPACE_RELATED`)
- `TrustExtension::EditorAdded/EditorRemoved`
- `TrustExtension::MemberAdded/MemberRemoved`
- DAO initial membership at `SpaceCreated`

**Graph inputs**:
- Create/remove explicit edges from the source space to a target space.
- Editor/Member edges are explicit edge types and are traversed the same way as Verified/Related for canonical inclusion.

**Canonical impact**:
- Any space reachable from the root via explicit edges is canonical.
- Removal of an explicit edge can remove spaces from the canonical set if they are no longer reachable via any explicit path.

### 4) Topic Edges (Non-Canonical-Granting)
**Source events**:
- `TrustExtended::Subtopic` (from `SUBSPACE_TOPIC_DECLARED`)

**Graph inputs**:
- Creates an edge from a space to a topic ID.

**Canonical impact**:
- Topic edges never expand the canonical set.
- When the source space is canonical, topic edges attach only canonical members of that topic.

## Canonical Inclusion Rules
A space is included in the canonical set **iff**:
1) It is the root, or
2) It is reachable from the root via one or more explicit edges
   (`Verified`, `Related`, `Editor`, `Member`).

Topic membership and topic edges **do not** grant canonical inclusion.
They only add edges between already-canonical nodes.

## Event-to-Input Mapping

| Event/Input | Graph Mutation | Can Expand Canonical Set? |
| --- | --- | --- |
| `SpaceCreated` | Add space, add set topic membership | No |
| `TOPIC_SET` / `TOPIC_REMOVED` | Add/remove topic membership | No |
| `TrustExtended::Verified` | Add explicit edge (Verified) | Yes |
| `TrustExtended::Related` | Add explicit edge (Related) | Yes |
| `EditorAdded/Removed` | Add/remove explicit edge (Editor) | Yes |
| `MemberAdded/Removed` | Add/remove explicit edge (Member) | Yes |
| DAO initial editors/members | Add explicit edges (Editor/Member) | Yes |
| `TrustExtended::Subtopic` | Add topic edge | No |

## Notes
- Explicit edges are the only inputs that grant canonical reachability.
- Topic membership updates can change which canonical members are attached via topic edges, but cannot introduce new canonical nodes.
- Event ordering for add/remove actions follows the log order within each block.

## Open Questions
None.
