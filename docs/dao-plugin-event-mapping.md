# DAO Plugin Event Mapping Documentation

## Overview

This document describes the relationship between DAO, their plugins, and the events they emit in the Geo protocol. Understanding these relationships is crucial for correctly implementing indexers.

According to the [Geo contracts README](https://github.com/geobrowser/geo-contracts/blob/main/README.md), a Space is composed of a DAO and several plugins. The DAO holds assets and manages permissions, while plugins provide composable logic for specific actions.

## DAO Plugin Types

There are two types of spaces, each with different plugin configurations:

### Spaces with Governance (Standard/Public Spaces)
These spaces have collaborative decision-making and include:
- **Space Plugin** - Source of truth for content and subspaces
- **Main Voting Plugin** - Handles proposals and voting
- **Member Access Plugin** - Manages membership requests

### Spaces without Governance (Personal Spaces)
These spaces have simplified, direct execution and include:
- **Space Plugin** - Source of truth for content and subspaces
- **Personal Admin Plugin** - Enables immediate proposal execution

### Plugin Descriptions

1. **Space Plugin** (`space_address`)
   - Acts as the source of truth for the Space
   - Core plugin for managing space content and subspaces
   - Upgradeable contract
   - Present in both governance and non-governance spaces

2. **Main Voting Plugin** (`main_voting_address`)
   - Governance plugin for standard spaces
   - Handles proposal voting and execution
   - Only members/editors can create proposals
   - Requires qualified majority vote for execution
   - Only present in spaces with governance

3. **Member Access Plugin** (`member_access_address`)
   - Manages membership requests for public/governance spaces
   - Allows editors to approve/reject membership proposals
   - Adapted from Aragon's Multisig plugin
   - Only present in spaces with governance

4. **Personal Admin Plugin** (`personal_admin_address`)
   - Enables immediate proposal execution for personal spaces
   - Editors can apply proposals directly
   - Not upgradeable
   - Only present in spaces without governance

## Event-to-Plugin Mapping

### Events Emitted by Space Plugin (`space_address`)

| Event            | Handler Function        | Description                                   |
| ---------------- | ----------------------- | --------------------------------------------- |
| SubspaceAccepted | `map_subspaces_added`   | When a subspace is added to the DAO           |
| SubspaceRemoved  | `map_subspaces_removed` | When a subspace is removed from the DAO       |
| EditsPublished   | `map_edits_published`   | When content edits are published to the space |

### Events Emitted by Main Voting Plugin (`main_voting_address`)

| Event                         | Handler Function                        | Description                                     |
| ----------------------------- | --------------------------------------- | ----------------------------------------------- |
| ProposalExecuted              | `map_proposals_executed`                | When a proposal is successfully executed        |
| VoteCast                      | `map_votes_cast`                        | When a vote is cast on a proposal               |
| PublishEditsProposalCreated   | `map_publish_edits_proposals_created`   | When a proposal to publish edits is created     |
| AcceptSubspaceProposalCreated | `map_add_subspace_proposals_created`    | When a proposal to add a subspace is created    |
| RemoveSubspaceProposalCreated | `map_remove_subspace_proposals_created` | When a proposal to remove a subspace is created |

### Events with Conditional Emission (Depends on Governance Type)

These events are emitted by **EITHER** the `personal_admin_address` OR the `member_access_address`, depending on whether the space is personal or public:

#### Membership Management Events

| Event                      | Handler Function      | Emitted By               | Condition                  |
| -------------------------- | --------------------- | ------------------------ | -------------------------- |
| MemberAdded / MembersAdded | `map_members_added`   | `personal_admin_address` | If personal space          |
| MemberAdded / MembersAdded | `map_members_added`   | `member_access_address`  | If public/governance space |
| MemberRemoved              | `map_members_removed` | `personal_admin_address` | If personal space          |
| MemberRemoved              | `map_members_removed` | `member_access_address`  | If public/governance space |

#### Editor Management Events

| Event                      | Handler Function      | Emitted By               | Condition                  |
| -------------------------- | --------------------- | ------------------------ | -------------------------- |
| EditorAdded / EditorsAdded | `map_editors_added`   | `personal_admin_address` | If personal space          |
| EditorAdded / EditorsAdded | `map_editors_added`   | `member_access_address`  | If public/governance space |
| EditorRemoved              | `map_editors_removed` | `personal_admin_address` | If personal space          |
| EditorRemoved              | `map_editors_removed` | `member_access_address`  | If public/governance space |

#### Membership Proposal Events

| Event                       | Handler Function                      | Emitted By               | Condition                  |
| --------------------------- | ------------------------------------- | ------------------------ | -------------------------- |
| AddMemberProposalCreated    | `map_add_member_proposals_created`    | `personal_admin_address` | If personal space          |
| AddMemberProposalCreated    | `map_add_member_proposals_created`    | `member_access_address`  | If public/governance space |
| RemoveMemberProposalCreated | `map_remove_member_proposals_created` | `personal_admin_address` | If personal space          |
| RemoveMemberProposalCreated | `map_remove_member_proposals_created` | `member_access_address`  | If public/governance space |

#### Editor Proposal Events

| Event                       | Handler Function                      | Emitted By               | Condition                  |
| --------------------------- | ------------------------------------- | ------------------------ | -------------------------- |
| AddEditorProposalCreated    | `map_add_editor_proposals_created`    | `personal_admin_address` | If personal space          |
| AddEditorProposalCreated    | `map_add_editor_proposals_created`    | `member_access_address`  | If public/governance space |
| RemoveEditorProposalCreated | `map_remove_editor_proposals_created` | `personal_admin_address` | If personal space          |
| RemoveEditorProposalCreated | `map_remove_editor_proposals_created` | `member_access_address`  | If public/governance space |

## Implications for Substreams Store Implementation

### Store Design Requirements

1. **Bidirectional Mapping**: We need to store both:
   - DAO → Plugin mappings (for looking up plugins by DAO)
   - Plugin → DAO mappings (for validating events from plugin addresses)

2. **Governance Type Tracking**: We need to track whether a DAO is personal or public to correctly validate membership/editor events:
   - Store `dao:type:{dao_address}` → `"personal"` or `"public"`

3. **Multiple Plugin Validation**: For conditional events, we need to check BOTH possible plugin addresses:
   - For personal spaces: Check if event comes from `personal_admin_address`
   - For public spaces: Check if event comes from `member_access_address`

### Recommended Store Keys

```
# DAO to Plugin mappings
dao:space:{dao_address} → space_address
dao:voting:{dao_address} → main_voting_address
dao:member_access:{dao_address} → member_access_address
dao:personal_admin:{dao_address} → personal_admin_address

# Plugin to DAO reverse mappings
plugin:dao:{plugin_address} → dao_address

# DAO type for conditional validation
dao:type:{dao_address} → "personal" | "public"
```

### Validation Logic Example

For an event like `MemberAdded`:

1. Get the plugin address that emitted the event
2. Look up the DAO address from `plugin:dao:{plugin_address}`
3. Check the DAO type from `dao:type:{dao_address}`
4. If personal: Verify plugin matches `dao:personal_admin:{dao_address}`
5. If public: Verify plugin matches `dao:member_access:{dao_address}`
6. Only process event if validation passes

## Notes

- The `derive_dao_address_from_plugin` function in `indexer/src/preprocess.rs` is currently a placeholder and should be replaced with proper Store lookups
- Events from unregistered plugin addresses should be filtered out to maintain data integrity
- The Store should be populated when plugin creation events are detected (during space/plugin setup)

## References

- [Geo Browser Contracts README](https://github.com/geobrowser/geo-contracts/blob/main/README.md) - Official source of truth for the protocol architecture and plugin design
