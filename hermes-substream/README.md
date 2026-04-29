# Hermes Substream

Substream that filters and emits events from the Space Registry contract for the Hermes indexing architecture.

## Overview

This substream decodes the `Action` event from the Space Registry contract and provides both:
- **Raw actions** - for consumers who want full control over event interpretation
- **Pre-filtered typed events** - for simpler consumption of specific event types

## Building

```bash
# Build everything (generates protos, compiles WASM, creates .spkg)
substreams build

# Or just repack without recompiling
substreams pack --output-file hermes-substream.spkg
```

The `substreams build` command:
1. Generates Rust protobuf bindings from `proto/schema.proto` → `src/pb/`
2. Compiles the WASM binary
3. Creates the `.spkg` package

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
| `map_subspaces_verified` | `SubspaceVerifiedList` | Verified subspaces |
| `map_subspaces_related` | `SubspaceRelatedList` | Related subspaces |
| `map_subspaces_topic_declared` | `SubspaceTopicDeclaredList` | Topic declarations for subspaces |
| `map_space_types_declared` | `SpaceTypeDeclaredList` | Space type declarations (e.g., DAO_SPACE) |
| `map_spaces_cleared` | `SpaceClearedList` | Cleared space IDs |
| `map_proposal_settings_used` | `ProposalSettingsUsedList` | Proposal settings snapshots |
| `map_proposals_updated` | `ProposalUpdatedList` | Updated proposals |

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
substreams run hermes-substream.spkg map_edits_published,map_editors_added,map_subspaces_verified

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
| Topic Set | `GOVERNANCE.TOPIC_SET` |
| Edits Published | `GOVERNANCE.EDITS_PUBLISHED` |
| Content Flagged | `GOVERNANCE.FLAGGED` |
| Content Unflagged | `GOVERNANCE.UNFLAGGED` |
| Subspace Verified | `GOVERNANCE.SUBSPACE_VERIFIED` |
| Subspace Related | `GOVERNANCE.SUBSPACE_RELATED` |
| Subspace Topic Set | `GOVERNANCE.SUBSPACE_TOPIC_SET` |
| Space Type Declared | `GOVERNANCE.SPACE_TYPE_DECLARED` |
| Space ID Cleared | `GOVERNANCE.SPACE_ID_CLEARED` |
| Proposal Settings Used | `GOVERNANCE.PROPOSAL_SETTINGS_USED` |
| Proposal Updated | `GOVERNANCE.PROPOSAL_UPDATED` |
| Upvoted | `PERMISSIONLESS.UPVOTED` |
| Downvoted | `PERMISSIONLESS.DOWNVOTED` |
| Unvoted | `PERMISSIONLESS.UNVOTED` |

## Configuration

The Space Registry contract address is configured in `src/lib.rs` for the ZC16 testnet:

```rust
// Space Registry proxy contract address (ZC16 testnet)
const SPACE_REGISTRY_ADDRESS: [u8; 20] = [
    0x49, 0x2B, 0xFF, 0x74, 0xb1, 0x3A, 0xCF, 0x3C, 0xC2, 0x49, 0xA9, 0x8d, 0x07, 0x9F, 0x0a, 0x6F,
    0x1d, 0x07, 0xDD, 0x2f,
]; // 0x492BFF74b13ACF3cC249A98d079F0a6F1d07DD2f
```

## Development

See [docs/modifying-events.md](docs/modifying-events.md) for instructions on:
- Adding new events
- Modifying existing events
- Removing events
- Updating the ABI
- Testing changes
