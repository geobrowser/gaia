# Hermes Substream

Substream that filters and emits events from the Space Registry contract for the Hermes indexing architecture.

## Overview

This substream decodes the `Action` event from the Space Registry contract and provides both:
- **Raw actions** - for consumers who want full control over event interpretation
- **Pre-filtered typed events** - for simpler consumption of specific event types

## Building

```bash
# Build the WASM binary
cargo build --target wasm32-unknown-unknown --release

# Generate protobuf bindings
substreams protogen

# Pack into .spkg
substreams pack -o hermes-substream.spkg
```

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
| `map_content_flagged` | `ContentFlaggedList` | Flagged content |
| `map_subspaces_added` | `SubspaceAddedList` | Subspaces added to parent spaces |
| `map_subspaces_removed` | `SubspaceRemovedList` | Subspaces removed from parent spaces |

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
| Subspace Added | `GOVERNANCE.SUBSPACE_ADDED` |
| Subspace Removed | `GOVERNANCE.SUBSPACE_REMOVED` |
| Object Upvoted | `PERMISSIONLESS.OBJECT_UPVOTED` |
| Object Downvoted | `PERMISSIONLESS.OBJECT_DOWNVOTED` |
| Object Unvoted | `PERMISSIONLESS.OBJECT_UNVOTED` |

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
