# ADR 0001: Canonical Graph Inputs

## Status
Proposed

## Date
2026-01-21

## Context
We need a shared definition of which inputs should be allowed to include a space in the canonical graph and which inputs should only add auxiliary connections. This ADR is forward-looking and defines the intended target state. The existing Atlas implementation is a proof of concept and is not actively used in the protocol at the time of this ADR.

## Decision
### Canonical Root
- A single configured root space ID seeds canonical traversal.
- The root is always canonical and is included even if it has no edges.

### Space Lifecycle (Spaces + Topic Setting)
**Source events**:
- `SpaceCreated` (from `SPACE_REGISTERED` in substreams)
- `TOPIC_SET` / `TOPIC_REMOVED`

**Graph inputs**:
- Adds the space to the graph.
- Adds the space to topic membership for its set topic(s).

**Canonical impact**:
- Does **not** make the space canonical on its own.
- Only affects canonical output when a canonical node has a topic edge to that topic.

### Explicit Edges (Canonical-Granting)
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

### Topic Edges (Non-Canonical-Granting)
**Source events**:
- `TrustExtended::Subtopic` (from `SUBSPACE_TOPIC_DECLARED`)

**Graph inputs**:
- Creates an edge from a space to a topic ID.

**Canonical impact**:
- Topic edges never expand the canonical set.
- When the source space is canonical, topic edges attach only canonical members of that topic.

### Canonical Inclusion Rules
A space is included in the canonical set **iff**:
1) It is the root, or
2) It is reachable from the root via one or more explicit edges (`Verified`, `Related`, `Editor`, `Member`).

Topic membership and topic edges **do not** grant canonical inclusion.
They only add edges between already-canonical nodes.

### Event-to-Input Mapping

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

### Ordering
- Event ordering for add/remove actions follows the log order within each block.

## Consequences
- Explicit edges are the only inputs that grant canonical reachability.
- Topic membership updates can change which canonical members are attached via topic edges, but cannot introduce new canonical nodes.

## Open Questions
None.
