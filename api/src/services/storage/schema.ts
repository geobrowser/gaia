import {
	relations as drizzleRelations,
	sql,
	type InferSelectModel,
} from "drizzle-orm";
import {
	bigint,
	boolean,
	customType,
	decimal,
	index,
	jsonb,
	pgEnum,
	pgTable,
	primaryKey,
	serial,
	smallint,
	text,
	timestamp,
	unique,
	uuid,
	varchar,
} from "drizzle-orm/pg-core";

// Enable the pg_trgm extension for similarity searches (executed at runtime)
// This comment signals that we want the trigram extension available
// The actual extension creation is handled in migrations

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
});

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
export const meta = pgTable("meta", {
	id: text().primaryKey(),
	cursor: text().notNull(),
	blockNumber: text().notNull(),
});

export const spaceTypesEnum = pgEnum("spaceTypes", ["Personal", "Public"]);

export const spaces = pgTable("spaces", {
	id: uuid().primaryKey(),
	type: spaceTypesEnum().notNull(),
	daoAddress: text().notNull(),
	spaceAddress: text().notNull(),
	mainVotingAddress: text(),
	membershipAddress: text(),
	personalAddress: text(),
});

export const entities = pgTable(
	"entities",
	{
		id: uuid().primaryKey(),
		createdAt: text().notNull(),
		createdAtBlock: text().notNull(),
		updatedAt: text().notNull(),
		updatedAtBlock: text().notNull(),
	},
	(table) => [
		// Index for ordering queries
		index("entities_updated_at_idx").on(table.updatedAt),
		// Composite index for ordering with id for stable pagination
		index("entities_updated_at_id_idx").on(table.updatedAt, table.id),
	],
);

export const dataTypesEnum = pgEnum("dataTypes", [
	"String",
	"Number",
	"Boolean",
	"Time",
	"Point",
	"Relation",
]);

export const properties = pgTable(
	"properties",
	{
		id: uuid().primaryKey(),
		type: dataTypesEnum().notNull(),
	},
	(table) => [
		// Index for filtering by data type
		index("properties_type_idx").on(table.type),
	],
);

export const values = pgTable(
	"values",
	{
		id: text().primaryKey(),
		propertyId: uuid()
			.notNull()
			.references(() => properties.id),
		entityId: uuid()
			.notNull()
			.references(() => entities.id),
		spaceId: uuid()
			.notNull()
			.references(() => spaces.id),
		string: text(),
		boolean: boolean(),
		number: decimal(),
		point: text(),
		time: text(),
		language: text(),
		unit: text(),
	},
	(table) => [
		// Foreign key indexes for join performance
		index("values_property_id_idx").on(table.propertyId),
		index("values_entity_id_idx").on(table.entityId),
		index("values_space_id_idx").on(table.spaceId),

		// Partial B-tree index for text searches (only indexes strings ≤2000 chars)
		// Longer strings will use sequential scan, but won't cause index size errors
		index("values_text_idx")
			.on(table.string)
			.where(sql`length(${table.string}) <= 2000`),
		index("values_number_idx").on(table.number),
		index("values_point_idx").on(table.point),
		index("values_boolean_idx").on(table.boolean),
		index("values_time_idx").on(table.time),
		// GIN index creation is handled via migration

		// Composite indexes for common query patterns
		index("values_entity_property_idx").on(table.entityId, table.propertyId),
		index("values_entity_space_idx").on(table.entityId, table.spaceId),
		index("values_property_space_idx").on(table.propertyId, table.spaceId),
		index("values_entity_property_space_idx").on(
			table.entityId,
			table.propertyId,
			table.spaceId,
		),

		// Composite index for space-filtered searches
		// index("values_space_text_idx").on(table.spaceId, table.string),

		// Additional indexes for filtering
		index("values_language_idx").on(table.language),
		index("values_unit_idx").on(table.unit),
	],
);

export const relations = pgTable(
	"relations",
	{
		id: uuid().primaryKey(),
		entityId: uuid()
			.notNull()
			.references(() => entities.id),
		typeId: uuid()
			.notNull()
			.references(() => properties.id),
		fromEntityId: uuid()
			.notNull()
			.references(() => entities.id),
		fromSpaceId: uuid().references(() => spaces.id),
		fromVersionId: uuid(),
		toEntityId: uuid()
			.notNull()
			.references(() => entities.id),
		toSpaceId: uuid().references(() => spaces.id),
		toVersionId: uuid(),
		position: text(),
		spaceId: uuid()
			.notNull()
			.references(() => spaces.id),
		verified: boolean(),
	},
	(table) => [
		// Foreign key indexes for join performance
		index("relations_entity_id_idx").on(table.entityId),
		index("relations_type_id_idx").on(table.typeId),
		index("relations_from_entity_id_idx").on(table.fromEntityId),
		index("relations_to_entity_id_idx").on(table.toEntityId),
		index("relations_space_id_idx").on(table.spaceId),

		// Composite indexes for common query patterns
		index("relations_space_from_to_idx").on(
			table.spaceId,
			table.fromEntityId,
			table.toEntityId,
		),
		index("relations_space_type_idx").on(table.spaceId, table.typeId),
		index("relations_to_entity_space_idx").on(table.toEntityId, table.spaceId),
		index("relations_from_entity_space_idx").on(
			table.fromEntityId,
			table.spaceId,
		),

		// Additional composite indexes for complex queries
		index("relations_entity_type_space_idx").on(
			table.entityId,
			table.typeId,
			table.spaceId,
		),
		index("relations_type_from_to_idx").on(
			table.typeId,
			table.fromEntityId,
			table.toEntityId,
		),
	],
);

export const members = pgTable(
	"members",
	{
		address: text().notNull(),
		spaceId: uuid()
			.notNull()
			.references(() => spaces.id),
	},
	(table) => [
		primaryKey({ columns: [table.address, table.spaceId] }),
		index("members_space_id_idx").on(table.spaceId),
	],
);

export const editors = pgTable(
	"editors",
	{
		address: text().notNull(),
		spaceId: uuid()
			.notNull()
			.references(() => spaces.id),
	},
	(table) => [
		primaryKey({ columns: [table.address, table.spaceId] }),
		index("editors_space_id_idx").on(table.spaceId),
	],
);

export const subspaces = pgTable(
	"subspaces",
	{
		parentSpaceId: uuid()
			.notNull()
			.references(() => spaces.id),
		childSpaceId: uuid()
			.notNull()
			.references(() => spaces.id),
	},
	(table) => [
		primaryKey({ columns: [table.parentSpaceId, table.childSpaceId] }),
		index("subspaces_parent_space_id_idx").on(table.parentSpaceId),
		index("subspaces_child_space_id_idx").on(table.childSpaceId),
	],
);

export const entityForeignValues = drizzleRelations(
	entities,
	({ many, one }) => ({
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
	}),
);

export const propertiesEntityRelations = drizzleRelations(
	values,
	({ one }) => ({
		entity: one(entities, {
			fields: [values.entityId],
			references: [entities.id],
		}),
	}),
);

export const propertiesRelations = drizzleRelations(
	properties,
	({ one, many }) => ({
		entity: one(entities, {
			fields: [properties.id],
			references: [entities.id],
		}),
		// Relations where this property is used as the type
		typeRelations: many(relations, {
			relationName: "typeProperty",
		}),
	}),
);

export const relationsEntityRelations = drizzleRelations(
	relations,
	({ one }) => ({
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
	}),
);

export const membersRelations = drizzleRelations(members, ({ one }) => ({
	space: one(spaces, {
		fields: [members.spaceId],
		references: [spaces.id],
	}),
}));

export const editorsRelations = drizzleRelations(editors, ({ one }) => ({
	space: one(spaces, {
		fields: [editors.spaceId],
		references: [spaces.id],
	}),
}));

export const subspacesRelations = drizzleRelations(subspaces, ({ one }) => ({
	parentSpace: one(spaces, {
		fields: [subspaces.parentSpaceId],
		references: [spaces.id],
		relationName: "parentSpace",
	}),
	childSpace: one(spaces, {
		fields: [subspaces.childSpaceId],
		references: [spaces.id],
		relationName: "childSpace",
	}),
}));

export const spacesRelations = drizzleRelations(spaces, ({ many }) => ({
	members: many(members),
	editors: many(editors),
	parentSpaces: many(subspaces, {
		relationName: "childSpace",
	}),
	childSpaces: many(subspaces, {
		relationName: "parentSpace",
	}),
}));

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>;
export type DbEntity = InferSelectModel<typeof entities>;
export type DbProperty = InferSelectModel<typeof values>;
export type DbRelations = InferSelectModel<typeof relations>;
export type DbMember = InferSelectModel<typeof members>;
export type DbEditor = InferSelectModel<typeof editors>;

/** Actions Schema definitions */

/**
 * bytea
 *
 * This is a custom type for the bytea data type.
 * It is used to store binary data in the database.
 */
export const bytea = customType<{
	data: Buffer;
	default: false;
}>({
	dataType() {
		return "bytea";
	},
});

/**
 * raw_actions
 */
export const rawActions = pgTable(
	"raw_actions",
	{
		id: serial("id").primaryKey(),
		actionType: bigint("action_type", { mode: "number" }).notNull(),
		actionVersion: bigint("action_version", { mode: "number" }).notNull(),
		sender: varchar("sender", { length: 42 }).notNull(),
		objectId: uuid("object_id").notNull(),
		groupId: uuid("group_id"),
		spacePov: uuid("space_pov").notNull(),
		metadata: bytea("metadata"),
		blockNumber: bigint("block_number", { mode: "number" }).notNull(),
		blockTimestamp: timestamp("block_timestamp", {
			withTimezone: true,
			mode: "date",
		}).notNull(),
		txHash: varchar("tx_hash", { length: 66 }).notNull(),
		objectType: smallint("object_type").notNull(),
	},
	// no explicit indexes/uniques defined in SQL for this table
);

/**
 * user_votes
 */
export const userVotes = pgTable(
	"user_votes",
	{
		id: serial("id").primaryKey(),
		userId: varchar("user_id", { length: 42 }).notNull(),
		objectId: uuid("object_id").notNull(),
		objectType: smallint("object_type").notNull(),
		spaceId: uuid("space_id").notNull(),
		voteType: smallint("vote_type").notNull(),
		votedAt: timestamp("voted_at", {
			withTimezone: true,
			mode: "date",
		}).notNull(),
	},
	(table) => {
		return {
			// UNIQUE(user_id, object_id, space_id)
			uqUserEntityObjectTypeSpace: unique("user_votes_user_entity_object_type_space_unique").on(
				table.userId,
				table.objectId,
				table.objectType,
				table.spaceId,
			),
			// CREATE INDEX idx_user_votes_user_entity_space ON user_votes(user_id, object_id, space_id)
			idxUserEntityObjectTypeSpace: index("idx_user_votes_user_entity_object_type_space").on(
				table.userId,
				table.objectId,
				table.objectType,
				table.spaceId,
			),
		};
	},
);

/**
 * votes_count
 */
export const votesCount = pgTable(
	"votes_count",
	{
		id: serial("id").primaryKey(),
		objectId: uuid("object_id").notNull(),
		objectType: smallint("object_type").notNull(),
		spaceId: uuid("space_id").notNull(),
		upvotes: bigint("upvotes", { mode: "number" }).notNull().default(0),
		downvotes: bigint("downvotes", { mode: "number" }).notNull().default(0),
	},
	(table) => {
		return {
			// UNIQUE(object_id, object_type, space_id)
			uqObjectObjectTypeSpace: unique("votes_count_object_object_type_space_unique").on(
				table.objectId,
				table.objectType,
				table.spaceId,
			),
			// CREATE INDEX idx_votes_count_space ON votes_count(space_id)
			idxSpace: index("idx_votes_count_space").on(table.spaceId),
			// CREATE INDEX idx_votes_count_entity_space ON votes_count(object_id, object_type, space_id)
			idxObjectObjectTypeSpace: index("idx_votes_count_object_object_type_space").on(
				table.objectId,
				table.objectType,
				table.spaceId,
			),
		};
	},
);
