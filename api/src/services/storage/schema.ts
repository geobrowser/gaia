import {type InferSelectModel, relations as drizzleRelations} from "drizzle-orm"
import {boolean, index, jsonb, pgEnum, pgTable, serial, text, uuid} from "drizzle-orm/pg-core"

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

export const entityForeignValues = drizzleRelations(entities, ({many}) => ({
	values: many(values),
	fromRelations: many(relations, {
		relationName: "fromEntity",
	}),
	// If an entity is the object (i.e. toEntity)
	toRelations: many(relations, {
		relationName: "toEntity",
	}),
	// If an entity is the type of relation
	typeRelations: many(relations, {
		relationName: "typeEntity",
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
	typeEntity: one(entities, {
		fields: [relations.typeId],
		references: [entities.id],
		relationName: "typeEntity",
	}),
	entity: one(entities, {
		fields: [relations.entityId],
		references: [entities.id],
		relationName: "entity",
	}),
}))

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>
export type DbEntity = InferSelectModel<typeof entities>
export type DbProperty = InferSelectModel<typeof values>
export type DbRelations = InferSelectModel<typeof relations>
