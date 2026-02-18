---
date: 2026-02-03
topic: proposal-names
---

# Proposal Names

## What We're Building

Add human-readable names to proposals. The name is derived from the proposal's actions:
- **Publish actions**: Use the edit's `name` field (fetched from IPFS cache)
- **Other actions**: Use the action type (e.g., "Add Member", "Flag", "Update Voting Settings")
- **Multiple actions**: Concatenate with comma separator (e.g., "Add Member, My Edit Name")

## Why This Approach

The edit name is already decoded by ipfs-indexer. Rather than duplicating IPFS decoding logic in multiple places, we:
1. Store the name in `ipfs_cache` at decode time
2. Read it in hermes-pipeline when constructing proposal events
3. Store the final concatenated name in kg-indexer's `proposals` table

This keeps the IPFS decoding responsibility in one place (ipfs-indexer) and flows the data through the existing event pipeline.

## Key Decisions

1. **Name stored on `PublishAction` proto**: The edit name is added to the `PublishAction` message in hermes-schema, not to `HermesProposalCreated`. This keeps the action-specific data with the action.

2. **Name stored in `proposals` table**: kg-indexer concatenates all action names and stores the result in `proposals.name`. This provides a single queryable field for proposal display.

3. **ipfs_cache stores name**: ipfs-indexer already decodes edits; it will also extract and store the `name` field. hermes-pipeline reads this when building proposal events.

4. **Concatenation format**: Multiple actions use comma separator (e.g., "Flag, Unflag, My Edit Name").

## Changes Required

### 1. ipfs-indexer
- Add `name TEXT` column to `ipfs_cache` table
- Extract and store `name` when decoding edits

### 2. hermes-schema
- Add `name` field to `PublishAction` proto message

### 3. hermes-pipeline
- Modify governance pipeline to read edit name from ipfs_cache for Publish actions
- Populate `PublishAction.name` field

### 4. api (schema.ts)
- Add `name TEXT` column to `proposals` table

### 5. kg-indexer
- When processing `HermesProposalCreated`/`HermesProposalUpdated`:
  - Concatenate action names (edit name for Publish, action type for others)
  - Store in `proposals.name`

## Open Questions

- None identified

## Next Steps

Run `/workflows:plan` for implementation details.
