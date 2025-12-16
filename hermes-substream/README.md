# Hermes Substream

Substream that filters and emits events from the Space Registry contract for the Hermes indexing architecture.

## Overview

This substream decodes the `Action` event from the Space Registry contract and provides both:
- **Raw actions** - for consumers who want full control over event interpretation
- **Pre-filtered typed events** - for simpler consumption of specific event types

## Building

```bash
# 1. Generate protobuf bindings
substreams protogen ./substreams.yaml --generate-mod-rs

# 2. Build the WASM binary
cargo build --release --target wasm32-unknown-unknown

# 3. Pack into .spkg
substreams pack -o hermes-substream.spkg
```

### Prerequisites

- Rust with `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`
- [Substreams CLI](https://substreams.streamingfast.io/getting-started/installing-the-cli)
- [Buf CLI](https://buf.build/docs/installation)

## Modules

### Raw Actions

| Module | Output | Description |
|--------|--------|-------------|
| `map_actions` | `Actions` | All raw Action events from Space Registry |

### Governance Events

| Module | Output | Description |
|--------|--------|-------------|
| `map_spaces_registered` | `SpaceRegisteredList` | New space registrations |
| `map_spaces_migrated` | `SpaceMigratedList` | Space migrations to new addresses |
| `map_proposals_created` | `ProposalCreatedList` | New governance proposals |
| `map_proposals_voted` | `ProposalVotedList` | Votes on proposals |
| `map_proposals_executed` | `ProposalExecutedList` | Executed proposals |
| `map_editors_added` | `EditorAddedList` | Editors added to spaces |
| `map_editors_removed` | `EditorRemovedList` | Editors removed from spaces |
| `map_members_added` | `MemberAddedList` | Members added to spaces |
| `map_members_removed` | `MemberRemovedList` | Members removed from spaces |
| `map_editors_flagged` | `EditorFlaggedList` | Flagged editors |
| `map_editors_unflagged` | `EditorUnflaggedList` | Unflagged editors |
| `map_spaces_left` | `SpaceLeftList` | Members leaving spaces |
| `map_topics_declared` | `TopicDeclaredList` | New topic declarations |
| `map_edits_published` | `EditsPublishedList` | Published edits |
| `map_flagged` | `FlaggedList` | Flagged content |
| `map_unflagged` | `UnflaggedList` | Unflagged content |
| `map_subspaces_added` | `SubspaceAddedList` | Subspaces added to parent spaces |
| `map_subspaces_removed` | `SubspaceRemovedList` | Subspaces removed from parent spaces |
| `map_subspaces_verified` | `SubspaceVerifiedList` | Verified subspaces |
| `map_subspaces_related` | `SubspaceRelatedList` | Related subspaces |
| `map_subspaces_topic_declared` | `SubspaceTopicDeclaredList` | Topic declarations for subspaces |

### Permissionless Events

| Module | Output | Description |
|--------|--------|-------------|
| `map_objects_upvoted` | `ObjectUpvotedList` | Upvotes on objects |
| `map_objects_downvoted` | `ObjectDownvotedList` | Downvotes on objects |
| `map_objects_unvoted` | `ObjectUnvotedList` | Removed votes on objects |

## Usage

Consumers specify which module(s) to subscribe to:

```bash
# Subscribe to a single module
substreams run hermes-substream.spkg map_edits_published

# Subscribe to multiple modules
substreams run hermes-substream.spkg map_edits_published,map_editors_added,map_subspaces_added

# Subscribe to all raw actions
substreams run hermes-substream.spkg map_actions
```

Only the requested modules are executed - subscribing to `map_edits_published` won't process or return data from other modules.

## Action Types

Events are identified by keccak256 hashes of action name strings:

| Action | Hash Source |
|--------|-------------|
| Space Registered | `GOVERNANCE.SPACE_ID_REGISTERED` |
| Space Migrated | `GOVERNANCE.SPACE_ID_MIGRATED` |
| Proposal Created | `GOVERNANCE.PROPOSAL_CREATED` |
| Proposal Voted | `GOVERNANCE.PROPOSAL_VOTED` |
| Proposal Executed | `GOVERNANCE.PROPOSAL_EXECUTED` |
| Editor Added | `GOVERNANCE.EDITOR_ADDED` |
| Editor Removed | `GOVERNANCE.EDITOR_REMOVED` |
| Member Added | `GOVERNANCE.MEMBER_ADDED` |
| Member Removed | `GOVERNANCE.MEMBER_REMOVED` |
| Editor Flagged | `GOVERNANCE.EDITOR_FLAGGED` |
| Editor Unflagged | `GOVERNANCE.EDITOR_UNFLAGGED` |
| Space Left | `GOVERNANCE.SPACE_LEFT` |
| Topic Declared | `GOVERNANCE.TOPIC_DECLARED` |
| Edits Published | `GOVERNANCE.EDITS_PUBLISHED` |
| Content Flagged | `GOVERNANCE.FLAGGED` |
| Content Unflagged | `GOVERNANCE.UNFLAGGED` |
| Subspace Added | `GOVERNANCE.SUBSPACE_ADDED` |
| Subspace Removed | `GOVERNANCE.SUBSPACE_REMOVED` |
| Subspace Verified | `GOVERNANCE.SUBSPACE_VERIFIED` |
| Subspace Related | `GOVERNANCE.SUBSPACE_RELATED` |
| Subspace Topic Declared | `GOVERNANCE.SUBSPACE_TOPIC_DECLARED` |
| Upvoted | `PERMISSIONLESS.UPVOTED` |
| Downvoted | `PERMISSIONLESS.DOWNVOTED` |
| Unvoted | `PERMISSIONLESS.UNVOTED` |

## Configuration

The Space Registry contract address is configured in `src/lib.rs`:

```rust
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [0u8; 20]; // TODO: Set actual address
```

## Development

See [docs/modifying-events.md](docs/modifying-events.md) for instructions on:
- Adding new events
- Modifying existing events
- Removing events
- Updating the ABI
- Testing changes
