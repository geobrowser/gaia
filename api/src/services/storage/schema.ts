import {relations as drizzleRelations, type InferSelectModel} from "drizzle-orm"
import {boolean, index, integer, jsonb, pgEnum, pgTable, primaryKey, serial, text, uuid} from "drizzle-orm/pg-core"

export const ipfsCache = pgTable("ipfs_cache", {
	id: serial(),
	json: jsonb(),
	uri: text().notNull().unique(),
	/**
	 * Sometimes an IPFS fetch can fail for multiple reasons. Primarily
	 * we care about cases where we fetched it correctly but it was in
	 * an incorrect format. We need to signal to consumers that the cache
	 * has the IPFS CID, but was unable to parse it.
	 */
	isErrored: boolean().notNull().default(false),
	block: text().notNull(),
	space: uuid().notNull(),
})

/**
 * Cursors store the latest indexed block log. Indexers store their latest
 * block log after they have completed indexing a block, and read the latest
 * block log when starting a new indexing process.
 *
 * The knowledge graph is a state machine, so block indexing should be
 * deterministic and idempotent to avoid writing data to the knowledge graph
 * which might disrupt its state.
 *
 * Currently, indexers may share databases, so the id for a given indexer
 * should be unique so they can query their cursor state appropriately. For
 * example, the kg indexer may use an id of "kg_indexer", and the ipfs cache
 * indexer may use "ipfs_indexer"
 */
export const cursors = pgTable("cursors", {
	id: text().primaryKey(),
	cursor: text().notNull(),
})

export const spaceTypesEnum = pgEnum("spaceTypes", ["Personal", "Public"])

export const spaces = pgTable("spaces", {
	id: uuid().primaryKey(),
	type: spaceTypesEnum().notNull(),
	daoAddress: text().notNull(),
	spaceAddress: text().notNull(),
	mainVotingAddress: text(),
	membershipAddress: text(),
	personalAddress: text(),
})

export const entities = pgTable("entities", {
	id: uuid().primaryKey(),
	createdAt: text().notNull(),
	createdAtBlock: text().notNull(),
	updatedAt: text().notNull(),
	updatedAtBlock: text().notNull(),
})

export const dataTypesEnum = pgEnum("dataTypes", ["Text", "Number", "Checkbox", "Time", "Point", "Relation"])

export const properties = pgTable("properties", {
	id: uuid().primaryKey(),
	type: dataTypesEnum().notNull(),
})

export const values = pgTable(
	"values",
	{
		id: text().primaryKey(),
		propertyId: uuid().notNull(),
		entityId: uuid().notNull(),
		spaceId: text().notNull(),
		value: text().notNull(),
		language: text(),
		unit: text(),
	},
	(table) => [
		// Basic index for text searches - will add GIN via migration
		index("values_text_idx").on(table.value),
		// Composite index for space-filtered searches
		index("values_space_text_idx").on(table.spaceId, table.value),
	],
)

export const relations = pgTable("relations", {
	id: uuid().primaryKey(),
	entityId: uuid().notNull(),
	typeId: uuid().notNull(),
	fromEntityId: uuid().notNull(),
	fromSpaceId: uuid(),
	fromVersionId: uuid(),
	toEntityId: uuid().notNull(),
	toSpaceId: uuid(),
	toVersionId: uuid(),
	position: text(),
	spaceId: uuid().notNull(),
	verified: boolean(),
})

export const members = pgTable(
	"members",
	{
		address: text().notNull(),
		spaceId: uuid().notNull(),
	},
	(table) => [primaryKey({columns: [table.address, table.spaceId]})],
)

export const editors = pgTable(
	"editors",
	{
		address: text().notNull(),
		spaceId: uuid().notNull(),
	},
	(table) => [primaryKey({columns: [table.address, table.spaceId]})],
)

export const entityForeignValues = drizzleRelations(entities, ({many, one}) => ({
	values: many(values),
	property: one(properties, {
		fields: [entities.id],
		references: [properties.id],
	}),
	fromRelations: many(relations, {
		relationName: "fromEntity",
	}),
	// If an entity is the object (i.e. toEntity)
	toRelations: many(relations, {
		relationName: "toEntity",
	}),
	// If an entity is directly linked (e.g. as owning the relation row)
	relationEntityRelations: many(relations, {
		relationName: "entity",
	}),
}))

export const propertiesEntityRelations = drizzleRelations(values, ({one}) => ({
	entity: one(entities, {
		fields: [values.entityId],
		references: [entities.id],
	}),
}))

export const propertiesRelations = drizzleRelations(properties, ({one, many}) => ({
	entity: one(entities, {
		fields: [properties.id],
		references: [entities.id],
	}),
	// Relations where this property is used as the type
	typeRelations: many(relations, {
		relationName: "typeProperty",
	}),
}))

export const relationsEntityRelations = drizzleRelations(relations, ({one}) => ({
	fromEntity: one(entities, {
		fields: [relations.fromEntityId],
		references: [entities.id],
		relationName: "fromEntity",
	}),
	toEntity: one(entities, {
		fields: [relations.toEntityId],
		references: [entities.id],
		relationName: "toEntity",
	}),
	typeProperty: one(properties, {
		fields: [relations.typeId],
		references: [properties.id],
		relationName: "typeProperty",
	}),
	relationEntity: one(entities, {
		fields: [relations.entityId],
		references: [entities.id],
		relationName: "relationEntity",
	}),
}))

export const membersRelations = drizzleRelations(members, ({one}) => ({
	space: one(spaces, {
		fields: [members.spaceId],
		references: [spaces.id],
	}),
}))

export const editorsRelations = drizzleRelations(editors, ({one}) => ({
	space: one(spaces, {
		fields: [editors.spaceId],
		references: [spaces.id],
	}),
}))

export const spacesRelations = drizzleRelations(spaces, ({many}) => ({
	members: many(members),
	editors: many(editors),
}))

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>
export type DbEntity = InferSelectModel<typeof entities>
export type DbProperty = InferSelectModel<typeof values>
export type DbRelations = InferSelectModel<typeof relations>
export type DbMember = InferSelectModel<typeof members>
export type DbEditor = InferSelectModel<typeof editors>
