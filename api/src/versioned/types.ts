/**
 * Types for versioned entity responses.
 *
 * Entities are returned with their values, relations (excluding block relations),
 * and blocks (entities linked via BLOCKS relation type) grouped together.
 *
 * All UUID fields use the `NormalizedUuid` branded type to guarantee dashless
 * lowercase hex format at compile time. See `utils/uuid.ts` for details.
 */

import type {Profile} from "../profile/types"
import type {NormalizedUuid} from "../utils/uuid"

// ============================================================================
// Diff Chunks
// ============================================================================

/**
 * A single chunk in a text diff.
 * Matches the output format of the `diff` package.
 */
export interface DiffChunk {
	value: string
	added?: boolean
	removed?: boolean
}

// ============================================================================
// Values
// ============================================================================

/**
 * A value at a specific version (used in snapshots).
 * Field names match GRC-20 v2 data types.
 */
export interface VersionedValue {
	propertyId: NormalizedUuid
	spaceId: NormalizedUuid
	// Value columns (GRC-20 v2 data types) - only one will be set
	boolean?: boolean | null // BOOL
	integer?: number | null // INT64
	float?: number | null // FLOAT64
	decimal?: string | null // DECIMAL
	text?: string | null // TEXT
	bytes?: string | null // BYTES (base64 encoded)
	date?: string | null // DATE (ISO 8601)
	time?: string | null // TIME (ISO 8601)
	datetime?: string | null // DATETIME (ISO 8601)
	schedule?: unknown | null // SCHEDULE (RFC 5545)
	point?: string | null // POINT (WGS84)
	rect?: string | null // RECT (bounding box)
	embedding?: unknown | null // EMBEDDING
	// Metadata
	language?: string | null // For TEXT values only
	unit?: string | null // For numerical values (INT64, FLOAT64, DECIMAL)
	// Context metadata (for block grouping)
	contextRootId?: NormalizedUuid | null // Parent entity in edit context
	contextEdgeTypeId?: NormalizedUuid | null // Relation type from context edge
}

/**
 * Value types that support text diffing.
 */
export type TextValueType = "TEXT"

/**
 * Value types that use simple before/after comparison.
 */
export type SimpleValueType =
	| "BOOL"
	| "INT64"
	| "FLOAT64"
	| "DECIMAL"
	| "BYTES"
	| "DATE"
	| "TIME"
	| "DATETIME"
	| "SCHEDULE"
	| "POINT"
	| "RECT"
	| "EMBEDDING"

export type ValueType = TextValueType | SimpleValueType

/**
 * A text value change with pre-computed word diff.
 */
export interface TextValueChange {
	propertyId: NormalizedUuid
	propertyName?: string | null // Resolved human-readable name for propertyId
	spaceId: NormalizedUuid
	type: TextValueType
	before: string | null
	after: string | null
	diff: DiffChunk[]
}

/**
 * A simple value change with before/after values.
 */
export interface SimpleValueChange {
	propertyId: NormalizedUuid
	propertyName?: string | null // Resolved human-readable name for propertyId
	spaceId: NormalizedUuid
	type: SimpleValueType
	before: string | null
	after: string | null
}

export type ValueChange = TextValueChange | SimpleValueChange

// ============================================================================
// Relations
// ============================================================================

/**
 * A relation at a specific version (excluding block relations).
 */
export interface VersionedRelation {
	relationId: NormalizedUuid
	typeId: NormalizedUuid
	fromEntityId: NormalizedUuid
	fromSpaceId?: NormalizedUuid | null
	toEntityId: NormalizedUuid
	toSpaceId?: NormalizedUuid | null
	position?: string | null
	spaceId: NormalizedUuid
	verified?: boolean | null
	// Context metadata (for block grouping)
	contextRootId?: NormalizedUuid | null // Parent entity in edit context
	contextEdgeTypeId?: NormalizedUuid | null // Relation type from context edge
}

/**
 * A relation change.
 */
export interface RelationChange {
	relationId: NormalizedUuid
	typeId: NormalizedUuid
	typeName?: string | null // Resolved human-readable name for typeId
	spaceId: NormalizedUuid
	changeType: "ADD" | "REMOVE" | "UPDATE"
	before?: {
		toEntityId: NormalizedUuid
		toEntityName?: string | null // Resolved human-readable name for toEntityId
		toSpaceId?: NormalizedUuid | null
		position?: string | null
	} | null
	after?: {
		toEntityId: NormalizedUuid
		toEntityName?: string | null // Resolved human-readable name for toEntityId
		toSpaceId?: NormalizedUuid | null
		position?: string | null
	} | null
}

// ============================================================================
// Blocks
// ============================================================================

/**
 * A block snapshot - an entity linked via BLOCKS relation.
 */
export interface BlockSnapshot {
	id: NormalizedUuid
	values: VersionedValue[]
	relations: VersionedRelation[]
}

/**
 * A text block change with pre-computed diff.
 */
export interface TextBlockChange {
	id: NormalizedUuid
	type: "textBlock"
	before: string | null
	after: string | null
	diff: DiffChunk[]
}

/**
 * An image block change with before/after URLs.
 */
export interface ImageBlockChange {
	id: NormalizedUuid
	type: "imageBlock"
	before: string | null
	after: string | null
}

/**
 * A data block change with before/after names.
 */
export interface DataBlockChange {
	id: NormalizedUuid
	type: "dataBlock"
	before: string | null
	after: string | null
}

export type BlockChange = TextBlockChange | ImageBlockChange | DataBlockChange

// ============================================================================
// Entity Snapshots and Diffs
// ============================================================================

/**
 * An entity snapshot at a specific version.
 */
export interface EntitySnapshot {
	id: NormalizedUuid
	values: VersionedValue[]
	relations: VersionedRelation[] // Excludes block relations
	blocks: BlockSnapshot[]
}

/**
 * A diff between two versions of an entity.
 */
export interface EntityDiff {
	entityId: NormalizedUuid
	name: string | null
	values: ValueChange[]
	relations: RelationChange[]
	blocks: BlockChange[]
}

/**
 * Items in dynamic groups - can be block changes or full entity diffs.
 */
export type DynamicGroupItem = BlockChange | EntityDiff

/**
 * A grouped entity snapshot with hybrid mode support.
 *
 * - `blocks` is the static key for BLOCKS relation type
 * - `groupKeys` lists dynamic group keys present (for discoverability)
 * - Dynamic keys (e.g., relation type IDs) map to arrays of child snapshots
 */
export interface GroupedEntitySnapshot {
	id: NormalizedUuid
	values: VersionedValue[]
	relations: VersionedRelation[] // Excludes grouped relations
	blocks: BlockSnapshot[] // Static key for BLOCKS
	groupKeys: NormalizedUuid[] // Dynamic keys present (excluding "blocks")
	groups: Record<NormalizedUuid, BlockSnapshot[]> // Dynamic groups by relation type ID
}

/**
 * A grouped entity diff with hybrid mode support.
 *
 * - `blocks` is the static key for BLOCKS relation type changes
 * - `groupKeys` lists dynamic group keys present (for discoverability)
 * - Dynamic keys map to arrays of child changes
 */
export interface GroupedEntityDiff {
	entityId: NormalizedUuid
	name: string | null
	values: ValueChange[]
	relations: RelationChange[]
	blocks: BlockChange[] // Static key for BLOCKS
	groupKeys: NormalizedUuid[] // Dynamic keys present (excluding "blocks")
	groups: Record<NormalizedUuid, DynamicGroupItem[]> // Dynamic groups by relation type ID
}

// ============================================================================
// API Response Types
// ============================================================================

/**
 * Edit metadata attached to API responses.
 */
interface EditMetadata {
	editName: string | null
	createdById: NormalizedUuid | null
	createdBy: Profile | null
}

/**
 * Response for GET /versioned/entities/:id — entity snapshot with edit metadata.
 */
export type SnapshotResponse = EntitySnapshot & EditMetadata

/**
 * Response for GET /versioned/entities/:id/diff — grouped diff with edit metadata for both sides.
 *
 * Note: `groups` from GroupedEntityDiff is spread at root level in the JSON response,
 * so dynamic group keys appear as top-level fields.
 */
export type DiffResponse = Omit<GroupedEntityDiff, "groups"> &
	Record<NormalizedUuid, DynamicGroupItem[]> & {
		fromEditName: string | null
		fromCreatedById: NormalizedUuid | null
		fromCreatedBy: Profile | null
		toEditName: string | null
		toCreatedById: NormalizedUuid | null
		toCreatedBy: Profile | null
	}

// ============================================================================
// Version History
// ============================================================================

/**
 * A version row as returned from the database query (before profile enrichment).
 */
export interface VersionRow {
	editId: NormalizedUuid
	name: string | null
	createdById: NormalizedUuid | null
	blockNumber: string
	createdAt: string
}

/**
 * A version entry for API responses (with resolved creator profile).
 */
export interface VersionEntry extends VersionRow {
	createdBy: Profile | null
}

// ============================================================================
// Proposal Diffs
// ============================================================================

/**
 * Pagination cursor for proposal diffs.
 */
export interface ProposalDiffCursor {
	entityIndex: number // Index into sorted entity list
	totalEntities: number // Total entity count for consistency check
}

/**
 * Proposal status for diffing purposes.
 */
export type ProposalStatus = "active" | "closed" | "executed"

/**
 * Paginated response for single-proposal diffs.
 */
export interface PaginatedProposalDiff {
	proposalId: NormalizedUuid
	spaceId: NormalizedUuid
	proposalStatus: ProposalStatus
	entities: EntityDiff[]
	pagination: {
		cursor: string | null // Base64-encoded cursor for next page
		hasMore: boolean
		totalEntities: number
	}
}

/**
 * Base-state mode for grouped proposal diffs.
 * - "active": all proposals are active; base = current live KG state
 * - "historical": all proposals are closed/executed; base = versioned state before earliest edit
 */
export type GroupedProposalDiffMode = "active" | "historical"

/**
 * Paginated response for grouped (multi-proposal) diffs.
 */
export interface PaginatedGroupedProposalDiff {
	proposalIds: NormalizedUuid[]
	spaceId: NormalizedUuid
	mode: GroupedProposalDiffMode
	entities: EntityDiff[]
	pagination: {
		cursor: string | null
		hasMore: boolean
		totalEntities: number
	}
}
