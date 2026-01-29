/**
 * Types for versioned entity responses.
 *
 * Entities are returned with their values, relations (excluding block relations),
 * and blocks (entities linked via BLOCKS relation type) grouped together.
 */

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
	propertyId: string
	spaceId: string
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
	embedding?: unknown | null // EMBEDDING
	// Metadata
	language?: string | null // For TEXT values only
	unit?: string | null // For numerical values (INT64, FLOAT64, DECIMAL)
	// Context metadata (for block grouping)
	contextRootId?: string | null // Parent entity in edit context
	contextEdgeTypeId?: string | null // Relation type from context edge
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
	| "EMBEDDING"

export type ValueType = TextValueType | SimpleValueType

/**
 * A text value change with pre-computed word diff.
 */
export interface TextValueChange {
	propertyId: string
	spaceId: string
	type: TextValueType
	before: string | null
	after: string | null
	diff: DiffChunk[]
}

/**
 * A simple value change with before/after values.
 */
export interface SimpleValueChange {
	propertyId: string
	spaceId: string
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
	relationId: string
	typeId: string
	fromEntityId: string
	fromSpaceId?: string | null
	toEntityId: string
	toSpaceId?: string | null
	position?: string | null
	spaceId: string
	verified?: boolean | null
	// Context metadata (for block grouping)
	contextRootId?: string | null // Parent entity in edit context
	contextEdgeTypeId?: string | null // Relation type from context edge
}

/**
 * A relation change.
 */
export interface RelationChange {
	relationId: string
	typeId: string
	spaceId: string
	changeType: "ADD" | "REMOVE" | "UPDATE"
	before?: {
		toEntityId: string
		toSpaceId?: string | null
		position?: string | null
	} | null
	after?: {
		toEntityId: string
		toSpaceId?: string | null
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
	id: string
	values: VersionedValue[]
	relations: VersionedRelation[]
}

/**
 * A text block change with pre-computed diff.
 */
export interface TextBlockChange {
	id: string
	type: "textBlock"
	before: string | null
	after: string | null
	diff: DiffChunk[]
}

/**
 * An image block change with before/after URLs.
 */
export interface ImageBlockChange {
	id: string
	type: "imageBlock"
	before: string | null
	after: string | null
}

/**
 * A data block change with before/after names.
 */
export interface DataBlockChange {
	id: string
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
	id: string
	values: VersionedValue[]
	relations: VersionedRelation[] // Excludes block relations
	blocks: BlockSnapshot[]
}

/**
 * A diff between two versions of an entity.
 */
export interface EntityDiff {
	entityId: string
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
	id: string
	values: VersionedValue[]
	relations: VersionedRelation[] // Excludes grouped relations
	blocks: BlockSnapshot[] // Static key for BLOCKS
	groupKeys: string[] // Dynamic keys present (excluding "blocks")
	groups: Record<string, BlockSnapshot[]> // Dynamic groups by relation type ID
}

/**
 * A grouped entity diff with hybrid mode support.
 *
 * - `blocks` is the static key for BLOCKS relation type changes
 * - `groupKeys` lists dynamic group keys present (for discoverability)
 * - Dynamic keys map to arrays of child changes
 */
export interface GroupedEntityDiff {
	entityId: string
	name: string | null
	values: ValueChange[]
	relations: RelationChange[]
	blocks: BlockChange[] // Static key for BLOCKS
	groupKeys: string[] // Dynamic keys present (excluding "blocks")
	groups: Record<string, DynamicGroupItem[]> // Dynamic groups by relation type ID
}

// ============================================================================
// Version History
// ============================================================================

/**
 * A version entry for listing versions.
 */
export interface VersionEntry {
	editId: string
	blockNumber: string
	createdAt: string
}

// ============================================================================
// Proposal Diffs
// ============================================================================

/**
 * Pagination cursor for proposal diffs.
 */
export interface ProposalDiffCursor {
	entityIndex: number // Index into sorted entity list
}

/**
 * Proposal status for diffing purposes.
 */
export type ProposalStatus = "active" | "closed" | "executed"

/**
 * Paginated response for proposal diffs.
 */
export interface PaginatedProposalDiff {
	proposalId: string
	spaceId: string
	proposalStatus: ProposalStatus
	entities: EntityDiff[]
	pagination: {
		cursor: string | null // Base64-encoded cursor for next page
		hasMore: boolean
		totalEntities: number
	}
}
