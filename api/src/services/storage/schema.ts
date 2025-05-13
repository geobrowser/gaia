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

export const properties = pgTable("properties", {
	id: text().primaryKey(),
	attributeId: text().notNull(),
	entityId: text().notNull(),
	spaceId: text().notNull(),
	textValue: text(),
	numberValue: text(),
	booleanValue: boolean(),
	languageOption: text(),
	formatOption: text(),
	unitOption: text(),
	valueType: text().notNull(),
})

export const relations = pgTable("relations", {
	id: text().primaryKey(),
	typeId: text().notNull(),
	fromEntityId: text().notNull(),
	toEntityId: text().notNull(),
	toSpaceId: text(),
	index: text(),
	spaceId: text().notNull(),
})

export const entityForeignProperties = drizzleRelations(entities, ({many}) => ({
	properties: many(properties),
	relationsOut: many(relations),
}))

export const propertiesEntityRelations = drizzleRelations(properties, ({one}) => ({
	entity: one(entities, {
		fields: [properties.entityId],
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
}))

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>
export type DbEntity = InferSelectModel<typeof entities>
export type DbProperty = InferSelectModel<typeof properties>
export type DbRelations = InferSelectModel<typeof relations>
