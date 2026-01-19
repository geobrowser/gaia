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
	value: string;
	added?: boolean;
	removed?: boolean;
}

// ============================================================================
// Values
// ============================================================================

/**
 * A value at a specific version (used in snapshots).
 */
export interface VersionedValue {
	propertyId: string;
	spaceId: string;
	// Value data - only one will be set
	string?: string | null;
	boolean?: boolean | null;
	number?: string | null;
	time?: string | null;
	point?: string | null;
	language?: string | null;
	unit?: string | null;
	integer?: number | null;
	float?: number | null;
	bytes?: string | null; // base64 encoded
	date?: string | null;
	datetime?: string | null;
}

/**
 * Value types that support text diffing.
 */
export type TextValueType = "TEXT";

/**
 * Value types that use simple before/after comparison.
 */
export type SimpleValueType =
	| "NUMBER"
	| "BOOLEAN"
	| "TIME"
	| "POINT"
	| "DATE"
	| "DATETIME"
	| "BYTES";

export type ValueType = TextValueType | SimpleValueType;

/**
 * A text value change with pre-computed word diff.
 */
export interface TextValueChange {
	propertyId: string;
	spaceId: string;
	type: TextValueType;
	diff: DiffChunk[];
}

/**
 * A simple value change with before/after values.
 */
export interface SimpleValueChange {
	propertyId: string;
	spaceId: string;
	type: SimpleValueType;
	from: string | null;
	to: string | null;
}

export type ValueChange = TextValueChange | SimpleValueChange;

// ============================================================================
// Relations
// ============================================================================

/**
 * A relation at a specific version (excluding block relations).
 */
export interface VersionedRelation {
	relationId: string;
	typeId: string;
	fromEntityId: string;
	fromSpaceId?: string | null;
	toEntityId: string;
	toSpaceId?: string | null;
	position?: string | null;
	spaceId: string;
	verified?: boolean | null;
}

/**
 * A relation change.
 */
export interface RelationChange {
	relationId: string;
	typeId: string;
	spaceId: string;
	changeType: "ADD" | "REMOVE" | "UPDATE";
	from?: {
		toEntityId: string;
		toSpaceId?: string | null;
		position?: string | null;
	} | null;
	to?: {
		toEntityId: string;
		toSpaceId?: string | null;
		position?: string | null;
	} | null;
}

// ============================================================================
// Blocks
// ============================================================================

/**
 * A block snapshot - an entity linked via BLOCKS relation.
 */
export interface BlockSnapshot {
	id: string;
	values: VersionedValue[];
	relations: VersionedRelation[];
}

/**
 * A text block change with pre-computed diff.
 */
export interface TextBlockChange {
	id: string;
	type: "textBlock";
	diff: DiffChunk[];
}

/**
 * An image block change with before/after URLs.
 */
export interface ImageBlockChange {
	id: string;
	type: "imageBlock";
	from: string | null;
	to: string | null;
}

/**
 * A data block change with before/after names.
 */
export interface DataBlockChange {
	id: string;
	type: "dataBlock";
	from: string | null;
	to: string | null;
}

export type BlockChange = TextBlockChange | ImageBlockChange | DataBlockChange;

// ============================================================================
// Entity Snapshots and Diffs
// ============================================================================

/**
 * An entity snapshot at a specific version.
 */
export interface EntitySnapshot {
	id: string;
	values: VersionedValue[];
	relations: VersionedRelation[]; // Excludes block relations
	blocks: BlockSnapshot[];
}

/**
 * A diff between two versions of an entity.
 */
export interface EntityDiff {
	entityId: string;
	name: string | null;
	values: ValueChange[];
	relations: RelationChange[];
	blocks: BlockChange[];
}

// ============================================================================
// Version History
// ============================================================================

/**
 * A version entry for listing versions.
 */
export interface VersionEntry {
	editId: string;
	blockNumber: string;
	sequence: number;
	createdAt: string;
}
