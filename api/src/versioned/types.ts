/**
 * Types for versioned entity responses.
 *
 * Entities are returned with their values, relations (excluding block relations),
 * and blocks (entities linked via BLOCKS relation type) grouped together.
 */

/**
 * A value at a specific version.
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
	schedule?: unknown | null;
	embedding?: unknown | null;
}

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
 * A block snapshot - an entity linked via BLOCKS relation.
 */
export interface BlockSnapshot {
	id: string;
	values: VersionedValue[];
	relations: VersionedRelation[];
}

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
 * Changes to a set of values.
 */
export interface ValueChanges {
	added: VersionedValue[];
	removed: VersionedValue[];
	changed: Array<{
		propertyId: string;
		spaceId: string;
		from: VersionedValue;
		to: VersionedValue;
	}>;
}

/**
 * Changes to a set of relations.
 */
export interface RelationChanges {
	added: VersionedRelation[];
	removed: VersionedRelation[];
	changed: Array<{
		relationId: string;
		from: VersionedRelation;
		to: VersionedRelation;
	}>;
}

/**
 * Changes to a block.
 */
export interface BlockChange {
	id: string;
	values: ValueChanges;
	relations: RelationChanges;
}

/**
 * Changes to blocks.
 */
export interface BlockChanges {
	added: BlockSnapshot[];
	removed: string[]; // Block IDs
	changed: BlockChange[];
}

/**
 * A diff between two versions of an entity.
 */
export interface EntityDiff {
	entityId: string;
	values: ValueChanges;
	relations: RelationChanges;
	blocks: BlockChanges;
}

/**
 * A version entry for listing versions.
 */
export interface VersionEntry {
	editId: string;
	blockNumber: string;
	sequence: number;
	createdAt: string;
}
