import {type InferSelectModel, relations} from "drizzle-orm"
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

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>

export const entities = pgTable("entities", {
	id: text().primaryKey(),
	createdAt: text().notNull(),
	createdAtBlock: text().notNull(),
	updatedAt: text().notNull(),
	updatedAtBlock: text().notNull(),
})

export type DbEntity = InferSelectModel<typeof entities>

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

export const entityProperties = relations(entities, ({many}) => ({
	properties: many(properties),
}))

export const propertiesEntity = relations(properties, ({one}) => ({
	properties: one(entities, {
		fields: [properties.entityId],
		references: [entities.id],
	}),
}))

export type DbProperty = InferSelectModel<typeof properties>
