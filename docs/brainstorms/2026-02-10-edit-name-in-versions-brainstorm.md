# Brainstorm: Include Edit Name in Version Responses

**Date:** 2026-02-10
**Status:** Ready for planning

## What We're Building

Add the edit name to versioned API responses so consumers can display human-readable labels for versions/edits without a separate lookup.

**Scope:** All versioned endpoints except proposal diffs (which already handle edit names through their own parsing flow):
- `GET /versioned/entities/:id` (entity snapshot) - add edit name to response
- `GET /versioned/entities/:id/versions` (version list) - add name to each `VersionEntry`
- `GET /versioned/entities/:id/diff` (entity diff) - add edit names for from/to edits

## Why This Approach

**Store `name` on `edit_versions` table** rather than joining `ipfs_cache` at query time:

- The `HermesEdit` proto already carries a `name` field (tag 2) that's available when the kg-indexer creates `edit_versions` rows
- Storing it directly avoids runtime joins with `ipfs_cache` and coupling between the versioning system and the IPFS caching system
- The column is nullable since existing rows won't have names (backfill is optional/future)

## Key Decisions

1. **Add `name text` column to `edit_versions`** - update the Drizzle schema in `schema.ts` and run `bun db:generate` to create the migration. Nullable, no default, populated by the Rust indexer going forward
2. **Update `insert_edit_version` in Rust** to accept and write the name from `HermesEdit.name`
3. **Update `VersionEntry` type** to include `name: string | null`
4. **Update `resolveVersionKey` query** to also return the name alongside the version key
5. **Thread edit name through snapshot and diff endpoints** - add to response types where editId is referenced
6. **Proposal diffs excluded** - they already derive names from their own edit parsing flow

## Open Questions

- **Backfill:** Should we backfill names for existing `edit_versions` rows from `ipfs_cache`? (Can be a follow-up)
- **Empty names:** `HermesEdit.name` is a protobuf string (defaults to `""`). Should we store empty strings as NULL? (Probably yes, for consistency)
