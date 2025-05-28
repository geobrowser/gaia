import {type InferSelectModel, relations as drizzleRelations} from "drizzle-orm"
import {boolean, jsonb, pgTable, serial, text} from "drizzle-orm/pg-core"

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
	space: text().notNull(),
})

export const entities = pgTable("entities", {
	id: text().primaryKey(),
	createdAt: text().notNull(),
	createdAtBlock: text().notNull(),
	updatedAt: text().notNull(),
	updatedAtBlock: text().notNull(),
})

export const values = pgTable("values", {
	id: text().primaryKey(),
	propertyId: text().notNull(),
	entityId: text().notNull(),
	spaceId: text().notNull(),
	value: text().notNull(),
	language: text(),
	format: text(),
	unit: text(),
	timezone: text(),
	hasDate: boolean(),
	hasTime: boolean(),
})

export const relations = pgTable("relations", {
	id: text().primaryKey(),
	entityId: text().notNull(),
	typeId: text().notNull(),
	fromEntityId: text().notNull(),
	fromSpaceId: text(),
	fromVersionId: text(),
	toEntityId: text().notNull(),
	toSpaceId: text(),
	toVersionId: text(),
	position: text(),
	spaceId: text().notNull(),
	verified: boolean(),
})

export const entityForeignValues = drizzleRelations(entities, ({many}) => ({
	values: many(values),
	relations: many(relations),
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
	}),
	toEntity: one(entities, {
		fields: [relations.toEntityId],
		references: [entities.id],
	}),
	typeEntity: one(entities, {
		fields: [relations.typeId],
		references: [entities.id],
	}),
	entity: one(entities, {
		fields: [relations.entityId],
		references: [entities.id],
	}),
}))

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>
export type DbEntity = InferSelectModel<typeof entities>
export type DbProperty = InferSelectModel<typeof values>
export type DbRelations = InferSelectModel<typeof relations>
