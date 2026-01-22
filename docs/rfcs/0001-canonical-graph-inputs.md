# Canonical Graph Inputs (RFC)

*Date: 2026-01-21*

## Summary

This RFC defines which data inputs feed into canonical graph computation. Specifically, it answers: which events and derived edges can make a space canonical, and which only add connections between already-canonical nodes?

## Goals

We want to:
- Specify the event types that mutate canonical graph inputs
- Clarify which inputs can expand the canonical set vs. only add edges
- Align the Rust pipeline with the `@atlas` mock stream and storage semantics

## Non-Goals

This RFC doesn't cover:
- How the canonical graph algorithm works
- How changes to the canonical graph get emitted (see the Graph Diff Emission RFC)
- Kafka schema details or storage layout

## Context

This RFC is forward-looking—it describes the target state, not necessarily the current implementation. The existing Atlas code is a proof of concept that isn't actively used in the protocol yet.

## Definitions

A few terms we'll use throughout:

- **Canonical set**: All spaces reachable from the root via explicit edges
- **Explicit edges**: Edges that can grant canonical inclusion (Verified, Related, Editor, Member)
- **Topic edges**: Edges from a space to a topic—these only connect nodes that are already canonical
- **Topic membership**: The mapping of topic → spaces that have set that topic

## Inputs

### 1) Canonical Root

The canonical root is a single configured space ID that seeds the entire traversal. It's always canonical, even if it has no outgoing edges.

### 2) Space Lifecycle (Spaces + Topic Setting)

**Source events:**
- `SpaceCreated` (from `SPACE_REGISTERED` in substreams)
- `TOPIC_SET` / `TOPIC_REMOVED` (from `@atlas` mock stream)

**What happens:**
These events add the space to the graph and update topic membership for any topics the space has set.

**Canonical impact:**
Creating a space or setting a topic doesn't make a space canonical on its own. These events only matter when a canonical node later references that topic via a topic edge.

### 3) Explicit Edges (Canonical-Granting)

**Source events:**
- `TrustExtended::Verified` (from `SUBSPACE_VERIFIED`)
- `TrustExtended::Related` (from `SUBSPACE_RELATED`)
- `TrustExtension::EditorAdded/EditorRemoved`
- `TrustExtension::MemberAdded/MemberRemoved`
- DAO initial membership at `SpaceCreated`

**What happens:**
These create or remove explicit edges from a source space to a target space. Editor and Member edges work the same way as Verified and Related for canonical traversal.

**Canonical impact:**
This is the key one—any space reachable from the root via explicit edges becomes canonical. Removing an explicit edge can drop spaces from the canonical set if they're no longer reachable through any explicit path.

### 4) Topic Edges (Non-Canonical-Granting)

**Source events:**
- `TrustExtended::Subtopic` (from `SUBSPACE_TOPIC_DECLARED`)

**What happens:**
These create an edge from a space to a topic ID.

**Canonical impact:**
Topic edges never expand the canonical set. They only attach already-canonical members of that topic when the source space is itself canonical.

**Note:** While topic edges don't affect canonical inclusion, they can cause nodes to be reordered in the tree. A node might be discovered at a shorter distance via a topic edge than its original explicit-edge path, changing its position (and parent) in the canonical tree.

## Canonical Inclusion Rules

A space is canonical if and only if:
1. It's the root, or
2. It's reachable from the root via explicit edges (`Verified`, `Related`, `Editor`, `Member`)

Topic membership and topic edges never grant canonical inclusion—they only add edges between nodes that are already canonical.

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

- Explicit edges are the only path to canonical status
- Topic membership updates can change which canonical members get attached via topic edges, but can't introduce new canonical nodes
- Event ordering follows log order within each block
