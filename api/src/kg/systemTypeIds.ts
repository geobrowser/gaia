// System Type relation (system-minted, non-user-editable: the indexer always
// drops user attempts to author it). Single source of truth for the TS side —
// imported by the entities filter plugin and its tests. Not exported by
// @geoprotocol/geo-sdk@0.17.0, so pinned here; matches the Rust SDK's
// SYSTEM_TYPES_RELATION_TYPE_ID and the literal in drizzle migration
// 0065_system_type_ids.sql.
export const SYSTEM_TYPE_RELATION_TYPE_ID = "88b3d6ad-288c-529c-a212-0e1c24819185"
