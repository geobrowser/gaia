import { pgTable, uuid, text, unique, serial, jsonb, boolean, index, primaryKey, pgEnum } from "drizzle-orm/pg-core"
import { sql } from "drizzle-orm"

export const dataTypes = pgEnum("dataTypes", ['Text', 'Number', 'Checkbox', 'Time', 'Point', 'Relation'])
export const spaceTypes = pgEnum("spaceTypes", ['Personal', 'Public'])


export const entities = pgTable("entities", {
	id: uuid().primaryKey().notNull(),
	createdAt: text("created_at").notNull(),
	createdAtBlock: text("created_at_block").notNull(),
	updatedAt: text("updated_at").notNull(),
	updatedAtBlock: text("updated_at_block").notNull(),
});

export const ipfsCache = pgTable("ipfs_cache", {
	id: serial().notNull(),
	json: jsonb(),
	uri: text().notNull(),
	isErrored: boolean("is_errored").default(false).notNull(),
	block: text().notNull(),
	space: uuid().notNull(),
}, (table) => [
	unique("ipfs_cache_uri_unique").on(table.uri),
]);

export const properties = pgTable("properties", {
	id: uuid().primaryKey().notNull(),
	type: dataTypes().notNull(),
});

export const relations = pgTable("relations", {
	id: uuid().primaryKey().notNull(),
	entityId: uuid("entity_id").notNull(),
	typeId: uuid("type_id").notNull(),
	fromEntityId: uuid("from_entity_id").notNull(),
	fromSpaceId: uuid("from_space_id"),
	fromVersionId: uuid("from_version_id"),
	toEntityId: uuid("to_entity_id").notNull(),
	toSpaceId: uuid("to_space_id"),
	toVersionId: uuid("to_version_id"),
	position: text(),
	spaceId: uuid("space_id").notNull(),
	verified: boolean(),
});

export const spaces = pgTable("spaces", {
	id: uuid().primaryKey().notNull(),
	type: spaceTypes().notNull(),
	daoAddress: text("dao_address").notNull(),
	spaceAddress: text("space_address").notNull(),
	mainVotingAddress: text("main_voting_address"),
	membershipAddress: text("membership_address"),
	personalAddress: text("personal_address"),
});

export const values = pgTable("values", {
	id: text().primaryKey().notNull(),
	propertyId: uuid("property_id").notNull(),
	entityId: uuid("entity_id").notNull(),
	spaceId: text("space_id").notNull(),
	value: text().notNull(),
	language: text(),
	unit: text(),
}, (table) => [
	index("values_space_text_idx").using("btree", table.spaceId.asc().nullsLast().op("text_ops"), table.value.asc().nullsLast().op("text_ops")),
	index("values_text_idx").using("btree", table.value.asc().nullsLast().op("text_ops")),
]);

export const members = pgTable("members", {
	address: text().notNull(),
	spaceId: uuid("space_id").notNull(),
}, (table) => [
	primaryKey({ columns: [table.address, table.spaceId], name: "members_address_space_id_pk"}),
]);
