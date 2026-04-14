import {relations as drizzleRelations, type InferSelectModel, sql} from "drizzle-orm"
import {
	bigint,
	boolean,
	cidr,
	customType,
	decimal,
	doublePrecision,
	index,
	integer,
	jsonb,
	pgEnum,
	pgTable,
	primaryKey,
	serial,
	smallint,
	text,
	time,
	timestamp,
	unique,
	uuid,
} from "drizzle-orm/pg-core"

// Enable the pg_trgm extension for similarity searches (executed at runtime)
// This comment signals that we want the trigram extension available
// The actual extension creation is handled in migrations

/**
 * bytea
 *
 * This is a custom type for the bytea data type.
 * It is used to store binary data in the database.
 */
export const bytea = customType<{
	data: Buffer
	default: false
}>({
	dataType() {
		return "bytea"
	},
})

export const ipfsCache = pgTable("ipfs_cache", {
	id: serial(),
	data: bytea(),
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
	/**
	 * The edit name extracted from the GRC-20 payload.
	 * Used by hermes-pipeline to populate proposal names for Publish actions.
	 */
	name: text(),
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
export const meta = pgTable("meta", {
	id: text().primaryKey(),
	cursor: text().notNull(),
	blockNumber: text().notNull(),
})

/**
 * atlas_checkpoints
 *
 * Stores Atlas checkpoint state used for restart recovery.
 */
export const atlasCheckpoints = pgTable("atlas_checkpoints", {
	indexerId: text("indexer_id").primaryKey(),
	cursor: text().notNull(),
	blockNumber: bigint("block_number", {mode: "number"}).notNull(),
	graphStateVersion: smallint("graph_state_version").notNull(),
	runtimeCompatibilityMarker: text("runtime_compatibility_marker").notNull(),
	rootSpaceId: text("root_space_id").notNull(),
	schemaVersion: smallint("schema_version").notNull(),
	graphStateBlob: jsonb("graph_state_blob").notNull(),
	updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).notNull().defaultNow(),
})

export const spaceTypesEnum = pgEnum("spaceTypes", ["DAO", "Personal"])

export const spaces = pgTable("spaces", {
	id: uuid().primaryKey(),
	type: spaceTypesEnum().notNull(),
	address: text().notNull(),
	topicId: uuid().references(() => entities.id),
})

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
)

/**
 * values
 *
 * Stores property values for entities. Each value has a type-specific column populated
 * based on its GRC-20 data type (boolean, integer, text, datetime, etc.).
 *
 * Design: We optimize for normalization, user intent preservation, and query performance
 * over storage efficiency. For example, time/datetime values store both the original string
 * (with timezone offset) and a UTC-normalized column. This preserves the original
 * representation while enabling efficient time-based queries and comparisons.
 *
 * UTC normalization: The *_utc columns store UTC-normalized values. Rust writes the same
 * string to both columns, PostgreSQL casts to timestamptz/timetz (normalizing to UTC),
 * and the database timezone is set to UTC so reads return UTC.
 */
export const values = pgTable(
	"values",
	{
		id: text().primaryKey(),
		propertyId: uuid().notNull(),
		entityId: uuid().notNull(),
		spaceId: uuid().notNull(),
		// Value columns (GRC-20 v2 data types)
		boolean: boolean(), // BOOL
		integer: bigint({mode: "number"}), // INT64
		float: doublePrecision(), // FLOAT64
		decimal: decimal(), // DECIMAL
		text: text(), // TEXT
		bytes: bytea(), // BYTES
		date: text(), // DATE (ISO 8601)
		time: text(), // TIME (ISO 8601)
		datetime: text(), // DATETIME (ISO 8601)
		schedule: jsonb(), // SCHEDULE (RFC 5545)
		point: text(), // POINT (WGS84)
		rect: text(), // RECT (bounding box)
		embedding: jsonb(), // EMBEDDING
		// Metadata
		language: text(), // For TEXT values only
		unit: text(), // For numerical values (INT64, FLOAT64, DECIMAL)
		// UTC-normalized columns (Rust writes, PostgreSQL casts to UTC, DB timezone=UTC ensures UTC output)
		timeUtc: time("time_utc", {withTimezone: true}),
		datetimeUtc: timestamp("datetime_utc", {withTimezone: true, mode: "date"}),
	},
	(table) => [
		// Foreign key indexes for join performance
		index("values_property_id_idx").on(table.propertyId),
		index("values_entity_id_idx").on(table.entityId),
		index("values_space_id_idx").on(table.spaceId),

		// Value type indexes
		index("values_boolean_idx").on(table.boolean),
		index("values_integer_idx").on(table.integer),
		index("values_float_idx").on(table.float),
		index("values_decimal_idx").on(table.decimal),
		// Partial B-tree index for text searches (only indexes text ≤2000 chars)
		// Longer text will use sequential scan, but won't cause index size errors
		index("values_text_idx")
			.on(table.text)
			.where(sql`length(${table.text}) <= 2000`),
		index("values_date_idx").on(table.date),
		index("values_time_idx").on(table.time),
		index("values_datetime_idx").on(table.datetime),
		index("values_point_idx").on(table.point),
		index("values_rect_idx").on(table.rect),
		// GIN index for schedule and embedding handled via migration

		// Composite indexes for common query patterns
		index("values_entity_property_idx").on(table.entityId, table.propertyId),
		index("values_entity_space_idx").on(table.entityId, table.spaceId),
		index("values_property_space_idx").on(table.propertyId, table.spaceId),
		index("values_entity_property_space_idx").on(table.entityId, table.propertyId, table.spaceId),

		// Additional indexes for filtering
		index("values_language_idx").on(table.language),
		index("values_unit_idx").on(table.unit),
		// UTC time column indexes
		index("values_time_utc_idx").on(table.timeUtc),
		index("values_datetime_utc_idx").on(table.datetimeUtc),
	],
)

export const relations = pgTable(
	"relations",
	{
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
		isSystem: boolean().notNull().default(false),
	},
	(table) => [
		// Foreign key indexes for join performance
		index("relations_entity_id_idx").on(table.entityId),
		index("relations_type_id_idx").on(table.typeId),
		index("relations_from_entity_id_idx").on(table.fromEntityId),
		index("relations_to_entity_id_idx").on(table.toEntityId),
		index("relations_space_id_idx").on(table.spaceId),

		// Composite indexes for common query patterns
		index("relations_space_from_to_idx").on(table.spaceId, table.fromEntityId, table.toEntityId),
		index("relations_space_type_idx").on(table.spaceId, table.typeId),
		index("relations_to_entity_space_idx").on(table.toEntityId, table.spaceId),
		index("relations_from_entity_space_idx").on(table.fromEntityId, table.spaceId),

		// Additional composite indexes for complex queries
		index("relations_entity_type_space_idx").on(table.entityId, table.typeId, table.spaceId),
		index("relations_type_from_to_idx").on(table.typeId, table.fromEntityId, table.toEntityId),
	],
)

export const members = pgTable(
	"members",
	{
		memberSpaceId: uuid().notNull(),
		spaceId: uuid().notNull(),
	},
	(table) => [
		primaryKey({columns: [table.memberSpaceId, table.spaceId]}),
		index("members_space_id_idx").on(table.spaceId),
	],
)

export const editors = pgTable(
	"editors",
	{
		memberSpaceId: uuid().notNull(),
		spaceId: uuid().notNull(),
	},
	(table) => [
		primaryKey({columns: [table.memberSpaceId, table.spaceId]}),
		index("editors_space_id_idx").on(table.spaceId),
	],
)

export const subspaceTypeEnum = pgEnum("subspaceType", ["verified", "related"])

export const subspaces = pgTable(
	"subspaces",
	{
		parentSpaceId: uuid()
			.notNull()
			.references(() => spaces.id),
		childSpaceId: uuid()
			.notNull()
			.references(() => spaces.id),
		type: subspaceTypeEnum().notNull().default("verified"),
	},
	(table) => [
		primaryKey({columns: [table.parentSpaceId, table.childSpaceId, table.type]}),
		index("subspaces_parent_space_id_idx").on(table.parentSpaceId),
		index("subspaces_child_space_id_idx").on(table.childSpaceId),
	],
)

export const subspaceTopics = pgTable(
	"subspace_topics",
	{
		spaceId: uuid("space_id")
			.notNull()
			.references(() => spaces.id),
		topicId: uuid("topic_id")
			.notNull()
			.references(() => entities.id),
	},
	(table) => [
		primaryKey({columns: [table.spaceId, table.topicId]}),
		index("subspace_topics_space_id_idx").on(table.spaceId),
		index("subspace_topics_topic_id_idx").on(table.topicId),
	],
)

export const entityForeignValues = drizzleRelations(entities, ({many}) => ({
	values: many(values),
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
	// Properties are now entities, so typeId references the entities table
	typeEntity: one(entities, {
		fields: [relations.typeId],
		references: [entities.id],
		relationName: "typeEntity",
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
		relationName: "memberOf",
	}),
	memberSpace: one(spaces, {
		fields: [members.memberSpaceId],
		references: [spaces.id],
		relationName: "memberSpace",
	}),
}))

export const editorsRelations = drizzleRelations(editors, ({one}) => ({
	space: one(spaces, {
		fields: [editors.spaceId],
		references: [spaces.id],
		relationName: "editorOf",
	}),
	memberSpace: one(spaces, {
		fields: [editors.memberSpaceId],
		references: [spaces.id],
		relationName: "editorSpace",
	}),
}))

export const subspacesRelations = drizzleRelations(subspaces, ({one}) => ({
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
}))

export const subspaceTopicsRelations = drizzleRelations(subspaceTopics, ({one}) => ({
	space: one(spaces, {
		fields: [subspaceTopics.spaceId],
		references: [spaces.id],
		relationName: "subspaceTopicSpace",
	}),
	topic: one(entities, {
		fields: [subspaceTopics.topicId],
		references: [entities.id],
		relationName: "topic",
	}),
}))

export const spacesRelations = drizzleRelations(spaces, ({many, one}) => ({
	topic: one(entities, {
		fields: [spaces.topicId],
		references: [entities.id],
		relationName: "topic",
	}),
	members: many(members),
	editors: many(editors),
	parentSpaces: many(subspaces, {
		relationName: "childSpace",
	}),
	childSpaces: many(subspaces, {
		relationName: "parentSpace",
	}),
	subspaceTopics: many(subspaceTopics, {
		relationName: "subspaceTopicSpace",
	}),
}))

export type IpfsCacheItem = InferSelectModel<typeof ipfsCache>
export type DbEntity = InferSelectModel<typeof entities>
export type DbProperty = InferSelectModel<typeof values>
export type DbRelations = InferSelectModel<typeof relations>
export type DbMember = InferSelectModel<typeof members>
export type DbEditor = InferSelectModel<typeof editors>
export type DbGlobalScore = InferSelectModel<typeof globalScores>
export type DbLocalScore = InferSelectModel<typeof localScores>
export type DbSpaceScore = InferSelectModel<typeof spaceScores>

/** Governance Schema definitions */

export const votingModeEnum = pgEnum("votingMode", ["Fast", "Slow"])

export const voteOptionEnum = pgEnum("voteOption", ["Yes", "No", "Abstain"])

export const proposalActionTypeEnum = pgEnum("proposalActionType", [
	"AddMember",
	"RemoveMember",
	"AddEditor",
	"RemoveEditor",
	"UnflagEditor",
	"Publish",
	"Flag",
	"Unflag",
	"UpdateVotingSettings",
	"Unknown",
	// Subspace proposal actions
	"SubspaceVerified",
	"SubspaceUnverified",
	"SubspaceRelated",
	"SubspaceUnrelated",
	"SubspaceTopicDeclared",
	"SubspaceTopicRemoved",
	"SetTopic",
	"UnsetTopic",
])

/**
 * proposals
 *
 * Stores governance proposals for spaces.
 */
export const proposals = pgTable(
	"proposals",
	{
		id: uuid().primaryKey(),
		spaceId: uuid("space_id")
			.notNull()
			.references(() => spaces.id),
		proposedBy: uuid("proposed_by").notNull(),
		votingMode: votingModeEnum("voting_mode").notNull(),
		startTime: bigint("start_time", {mode: "number"}).notNull(),
		endTime: bigint("end_time", {mode: "number"}).notNull(),
		quorum: bigint("quorum", {mode: "number"}).notNull(),
		threshold: bigint("threshold", {mode: "number"}).notNull(),
		executedAt: bigint("executed_at", {mode: "number"}),
		createdAt: text("created_at").notNull(),
		createdAtBlock: text("created_at_block").notNull(),
		/**
		 * Human-readable name derived from the proposal's actions.
		 * For Publish actions, uses the edit name; for others, uses the action type.
		 * Multiple actions are concatenated with ", " separator.
		 */
		name: text(),
		/**
		 * Denormalized vote counts for efficient status filtering.
		 * Updated asynchronously by a background worker via proposal_tally_queue.
		 */
		yesCount: bigint("yes_count", {mode: "number"}).notNull().default(0),
		noCount: bigint("no_count", {mode: "number"}).notNull().default(0),
		abstainCount: bigint("abstain_count", {mode: "number"}).notNull().default(0),
	},
	(table) => [
		index("proposals_space_id_idx").on(table.spaceId),
		index("proposals_proposed_by_idx").on(table.proposedBy),
		index("proposals_created_at_idx").on(table.createdAt),
		// Composite indexes for list query ordering with pagination
		// Note: Single-column end_time index removed as it's redundant with composite index
		index("proposals_space_created_at_idx").on(table.spaceId, table.createdAt),
		index("proposals_space_end_time_idx").on(table.spaceId, table.endTime),
		index("proposals_space_start_time_idx").on(table.spaceId, table.startTime),
	],
)

/**
 * proposal_actions
 *
 * Stores actions within a governance proposal.
 * Each proposal can have multiple actions executed in sequence.
 * ID is deterministic (derived from proposal_id + index).
 * Payload fields are nullable - which fields are populated depends on action_type.
 */
export const proposalActions = pgTable(
	"proposal_actions",
	{
		id: uuid().primaryKey(),
		proposalId: uuid("proposal_id")
			.notNull()
			.references(() => proposals.id),
		actionType: proposalActionTypeEnum("action_type").notNull(),
		// Member/editor operations: AddMember, RemoveMember, AddEditor, RemoveEditor, UnflagEditor
		targetId: uuid("target_id"),
		// Publish action
		contentUri: text("content_uri"),
		metadata: bytea("metadata"),
		// Flag/Unflag actions
		contentId: bytea("content_id"),
		// UpdateVotingSettings action
		quorum: bigint("quorum", {mode: "number"}),
		fastThreshold: bigint("fast_threshold", {mode: "number"}),
		slowThreshold: bigint("slow_threshold", {mode: "number"}),
		duration: bigint("duration", {mode: "number"}),
	},
	(table) => [
		index("proposal_actions_proposal_id_idx").on(table.proposalId),
		index("proposal_actions_action_type_idx").on(table.actionType),
		// Composite index for the active proposal check query (hasActiveProposalForTarget)
		// which filters by (proposal_id, action_type, target_id) in a correlated EXISTS subquery
		index("proposal_actions_proposal_action_target_idx").on(table.proposalId, table.actionType, table.targetId),
	],
)

/**
 * proposal_votes
 *
 * Stores votes cast on governance proposals.
 */
export const proposalVotes = pgTable(
	"proposal_votes",
	{
		proposalId: uuid("proposal_id")
			.notNull()
			.references(() => proposals.id),
		voterId: uuid("voter_id").notNull(),
		spaceId: uuid("space_id")
			.notNull()
			.references(() => spaces.id),
		vote: voteOptionEnum("vote").notNull(),
		createdAt: text("created_at").notNull(),
		createdAtBlock: text("created_at_block").notNull(),
	},
	(table) => [
		primaryKey({columns: [table.proposalId, table.voterId]}),
		index("proposal_votes_space_id_idx").on(table.spaceId),
		index("proposal_votes_voter_id_idx").on(table.voterId),
		index("proposal_votes_vote_idx").on(table.vote),
	],
)

export const proposalsRelations = drizzleRelations(proposals, ({one, many}) => ({
	space: one(spaces, {
		fields: [proposals.spaceId],
		references: [spaces.id],
	}),
	actions: many(proposalActions),
	votes: many(proposalVotes),
}))

export const proposalActionsRelations = drizzleRelations(proposalActions, ({one}) => ({
	proposal: one(proposals, {
		fields: [proposalActions.proposalId],
		references: [proposals.id],
	}),
}))

export const proposalVotesRelations = drizzleRelations(proposalVotes, ({one}) => ({
	proposal: one(proposals, {
		fields: [proposalVotes.proposalId],
		references: [proposals.id],
	}),
	space: one(spaces, {
		fields: [proposalVotes.spaceId],
		references: [spaces.id],
	}),
}))

export type DbProposal = InferSelectModel<typeof proposals>
export type DbProposalAction = InferSelectModel<typeof proposalActions>
export type DbProposalVote = InferSelectModel<typeof proposalVotes>

/**
 * proposal_tally_queue
 *
 * Queue for proposals that need their vote tallies updated.
 * When a vote is cast, the proposal_id is added to this queue (inside the same transaction).
 * A background worker processes this queue and updates the denormalized vote counts on proposals.
 * This decouples the vote write path from the tally computation for better performance.
 */
export const proposalTallyQueue = pgTable("proposal_tally_queue", {
	proposalId: uuid("proposal_id")
		.primaryKey()
		.references(() => proposals.id, {onDelete: "cascade"}),
	queuedAt: timestamp("queued_at", {withTimezone: true, mode: "date"}).notNull().defaultNow(),
})

/** Scoring Tables */

/**
 * global_scores
 *
 * Stores the global score for each entity across all spaces.
 * Each entity has exactly one global score.
 */
export const globalScores = pgTable("global_scores", {
	entityId: uuid("entity_id").primaryKey(),
	score: decimal("score").notNull(),
	updatedAt: timestamp("updated_at", {
		withTimezone: true,
		mode: "date",
	}).notNull(),
})

/**
 * local_scores
 *
 * Stores the local score for an entity within a specific space.
 */
export const localScores = pgTable(
	"local_scores",
	{
		entityId: uuid("entity_id").notNull(),
		spaceId: uuid("space_id").notNull(),
		score: decimal("score").notNull(),
		updatedAt: timestamp("updated_at", {
			withTimezone: true,
			mode: "date",
		}).notNull(),
	},
	(table) => {
		return {
			pk: primaryKey({columns: [table.entityId, table.spaceId]}),
			idxSpaceId: index("idx_local_scores_space_id").on(table.spaceId),
			idxEntityId: index("idx_local_scores_entity_id").on(table.entityId),
			idxSpaceScore: index("idx_local_scores_space_score").on(table.spaceId, table.score),
		}
	},
)

/**
 * space_scores
 *
 * Stores the overall score for each space.
 * Each space has exactly one score.
 */
export const spaceScores = pgTable("space_scores", {
	spaceId: uuid("space_id").primaryKey(),
	score: decimal("score").notNull(),
	updatedAt: timestamp("updated_at", {
		withTimezone: true,
		mode: "date",
	}).notNull(),
})

/**
 * votes
 *
 * Stores votes cast on entities and relations.
 */
export const votes = pgTable(
	"votes",
	{
		id: serial("id").primaryKey(),
		voterId: uuid("voter_id").notNull(),
		objectId: uuid("object_id").notNull(),
		objectType: smallint("object_type").notNull(),
		spaceId: uuid("space_id").notNull(),
		vote: smallint("vote").notNull(),
		blockNumber: bigint("block_number", {mode: "number"}).notNull(),
		blockTimestamp: timestamp("block_timestamp", {
			withTimezone: true,
			mode: "date",
		}).notNull(),
		createdAt: timestamp("created_at", {
			withTimezone: true,
			mode: "date",
		})
			.notNull()
			.defaultNow(),
	},
	(table) => ({
		voterIdIdx: index("idx_votes_voter_id").on(table.voterId),
		objectTypeObjectIdSpaceIdIdx: index("idx_votes_object_type_object_id_space_id").on(
			table.objectType,
			table.objectId,
			table.spaceId,
		),
	}),
)

/**
 * user_votes
 *
 * Tracks the most recent vote a user has cast on a specific object.
 * This table is used to prevent users from casting multiple votes on the same object
 * and to easily query a user's voting history.
 */
export const userVotes = pgTable(
	"user_votes",
	{
		userId: uuid("user_id").notNull(),
		objectId: uuid("object_id").notNull(),
		objectType: smallint("object_type").notNull(),
		spaceId: uuid("space_id").notNull(),
		voteType: smallint("vote_type").notNull(),
		votedAt: timestamp("voted_at", {
			withTimezone: true,
			mode: "date",
		}).notNull(),
	},
	(table) => ({
		uniqueConstraint: unique().on(table.userId, table.objectId, table.objectType, table.spaceId),
		objectIdx: index("idx_user_votes_object").on(table.objectId, table.objectType, table.spaceId),
	}),
)

/**
 * votes_count
 *
 * An aggregate table that stores the total upvotes and downvotes for each object.
 * This table is updated by the vote-indexer to provide fast access to vote counts
 * without needing to aggregate from the raw `votes` table on every query.
 */
export const votesCount = pgTable(
	"votes_count",
	{
		id: serial("id").primaryKey(),
		objectId: uuid("object_id").notNull(),
		objectType: smallint("object_type").notNull(),
		spaceId: uuid("space_id").notNull(),
		upvotes: bigint("upvotes", {mode: "number"}).notNull().default(0),
		downvotes: bigint("downvotes", {mode: "number"}).notNull().default(0),
	},
	(table) => ({
		uniqueConstraint: unique().on(table.objectId, table.objectType, table.spaceId),
	}),
)

/** Versioned Entities Schema */

/**
 * edit_versions
 *
 * Lookup table to resolve edit_id -> version_key for temporal queries.
 * version_key is a packed bigint: (block_number << 32) | sequence
 */
export const editVersions = pgTable(
	"edit_versions",
	{
		editId: uuid("edit_id").primaryKey(),
		blockNumber: bigint("block_number", {mode: "bigint"}).notNull(),
		sequence: bigint("sequence", {mode: "number"}).notNull(),
		versionKey: bigint("version_key", {mode: "bigint"}).notNull(),
		createdAt: timestamp("created_at", {mode: "date"}).notNull(),
		/**
		 * Human-readable name for this edit, extracted from the GRC-20 payload.
		 * Nullable because existing rows predate this column and some edits may lack names.
		 */
		name: text("name"),
		/**
		 * The first author (creator) of this edit, extracted from HermesEdit.authors[0].
		 * Stored as a UUID. Nullable because existing rows predate this column and
		 * some edits may lack authors.
		 */
		createdById: uuid("created_by_id"),
	},
	(table) => [
		unique("edit_versions_block_sequence_unique").on(table.blockNumber, table.sequence),
		index("edit_versions_version_key_idx").on(table.versionKey),
	],
)

/**
 * value_versions
 *
 * Temporal versioned values. Each row represents a value's state during a version range.
 * When a value changes, the current row's valid_to_key is set and a new row is inserted.
 */
export const valueVersions = pgTable(
	"value_versions",
	{
		id: uuid("id").primaryKey(),
		entityId: uuid("entity_id").notNull(),
		propertyId: uuid("property_id").notNull(),
		spaceId: uuid("space_id").notNull(),
		validFromKey: bigint("valid_from_key", {mode: "bigint"}).notNull(),
		validToKey: bigint("valid_to_key", {mode: "bigint"}),
		// Value columns (GRC-20 v2 data types)
		boolean: boolean("boolean"), // BOOL
		integer: bigint("integer", {mode: "number"}), // INT64
		float: doublePrecision("float"), // FLOAT64
		decimal: decimal("decimal"), // DECIMAL
		text: text("text"), // TEXT
		bytes: bytea("bytes"), // BYTES
		date: text("date"), // DATE (ISO 8601)
		time: text("time"), // TIME (ISO 8601)
		datetime: text("datetime"), // DATETIME (ISO 8601)
		schedule: jsonb("schedule"), // SCHEDULE (RFC 5545)
		point: text("point"), // POINT (WGS84)
		rect: text("rect"), // RECT (bounding box)
		embedding: jsonb("embedding"), // EMBEDDING
		// Metadata
		language: text("language"), // For TEXT values only
		unit: text("unit"), // For numerical values (INT64, FLOAT64, DECIMAL)
		// UTC-normalized columns (Rust writes, PostgreSQL casts to UTC, DB timezone=UTC ensures UTC output)
		timeUtc: time("time_utc", {withTimezone: true}),
		datetimeUtc: timestamp("datetime_utc", {withTimezone: true, mode: "date"}),
		// GRC-20 edit context (for context-aware diff grouping)
		contextRootId: uuid("context_root_id"), // Parent entity in edit context
		contextEdgeTypeId: uuid("context_edge_type_id"), // Relation type from context edge
	},
	(table) => [
		index("value_versions_entity_idx").on(table.entityId),
		index("value_versions_lookup_idx").on(table.entityId, table.propertyId, table.spaceId),
		index("value_versions_open_idx")
			.on(table.entityId, table.propertyId, table.spaceId)
			.where(sql`${table.validToKey} IS NULL`),
		index("value_versions_range_idx").on(table.entityId, table.validFromKey),
		// Partial index for version discovery queries that scan valid_to_key
		// to find edits that deleted/superseded values on an entity
		index("value_versions_entity_space_valid_to_idx")
			.on(table.entityId, table.spaceId, table.validToKey)
			.where(sql`${table.validToKey} IS NOT NULL`),
	],
)

/**
 * relation_versions
 *
 * Temporal versioned relations. Each row represents a relation's state during a version range.
 * When a relation changes, the current row's valid_to_key is set and a new row is inserted.
 */
export const relationVersions = pgTable(
	"relation_versions",
	{
		id: uuid("id").primaryKey(),
		relationId: uuid("relation_id").notNull(),
		entityId: uuid("entity_id").notNull(),
		typeId: uuid("type_id").notNull(),
		fromEntityId: uuid("from_entity_id").notNull(),
		fromSpaceId: uuid("from_space_id"),
		toEntityId: uuid("to_entity_id").notNull(),
		toSpaceId: uuid("to_space_id"),
		position: text("position"),
		spaceId: uuid("space_id").notNull(),
		verified: boolean("verified"),
		validFromKey: bigint("valid_from_key", {mode: "bigint"}).notNull(),
		validToKey: bigint("valid_to_key", {mode: "bigint"}),
		// GRC-20 edit context (for context-aware diff grouping)
		contextRootId: uuid("context_root_id"), // Parent entity in edit context
		contextEdgeTypeId: uuid("context_edge_type_id"), // Relation type from context edge
	},
	(table) => [
		index("relation_versions_relation_idx").on(table.relationId),
		index("relation_versions_entity_idx").on(table.entityId),
		index("relation_versions_open_idx").on(table.relationId).where(sql`${table.validToKey} IS NULL`),
		index("relation_versions_range_idx").on(table.entityId, table.validFromKey),
		// Composite index for batch block relation discovery queries that filter on
		// from_entity_id + type_id with temporal range predicates
		index("relation_versions_from_entity_type_idx").on(table.fromEntityId, table.typeId, table.validFromKey),
		// Partial index for version discovery queries that scan valid_to_key
		// to find edits that deleted/superseded relations on an entity
		index("relation_versions_from_entity_valid_to_idx")
			.on(table.fromEntityId, table.validToKey)
			.where(sql`${table.validToKey} IS NOT NULL`),
	],
)

export type DbEditVersion = InferSelectModel<typeof editVersions>
export type DbValueVersion = InferSelectModel<typeof valueVersions>
export type DbRelationVersion = InferSelectModel<typeof relationVersions>

/** Notification Service Tables */

/**
 * app_webhooks
 *
 * Registered webhook endpoints for app servers that receive governance notifications.
 * Webhooks are manually seeded in the DB (no registration API in v1).
 */
export const appWebhooks = pgTable("app_webhooks", {
	id: uuid().primaryKey().defaultRandom(),
	appName: text("app_name").notNull().unique(),
	url: text().notNull(),
	secret: text().notNull(),
	createdAt: timestamp("created_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
	updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
})

/**
 * notification_outbox
 *
 * Stores governance notifications before delivery.
 * Written by the notification-indexer from Kafka events and periodic rejection polling.
 */
export const notificationOutbox = pgTable("notification_outbox", {
	id: uuid().primaryKey().defaultRandom(),
	idempotencyKey: text("idempotency_key").notNull().unique(),
	eventType: text("event_type").notNull(),
	payload: jsonb().notNull(),
	createdAt: timestamp("created_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
	updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
})

/**
 * notification_deliveries
 *
 * Tracks delivery attempts for each outbox entry to each webhook.
 * The delivery-worker polls for pending rows and POSTs with HMAC signatures.
 */
export const notificationDeliveries = pgTable(
	"notification_deliveries",
	{
		id: uuid().primaryKey().defaultRandom(),
		outboxId: uuid("outbox_id")
			.notNull()
			.references(() => notificationOutbox.id),
		webhookId: uuid("webhook_id")
			.notNull()
			.references(() => appWebhooks.id),
		status: text().notNull().default("pending"),
		attempts: smallint().notNull().default(0),
		lastError: text("last_error"),
		nextRetryAt: timestamp("next_retry_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
		deliveredAt: timestamp("delivered_at", {withTimezone: true, mode: "date"}),
		createdAt: timestamp("created_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
		updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
	},
	(table) => [
		unique().on(table.outboxId, table.webhookId),
		index("idx_deliveries_pending").on(table.status, table.nextRetryAt),
	],
)

/**
 * rate_limit_overrides
 *
 * Per-IP and per-CIDR overrides for the GraphQL HTTP rate limiter. Rows here
 * take precedence over the env-configured default. Bare IPs may be inserted as
 * `'1.2.3.4'` (Postgres `cidr` widens to `/32`) or `'1.2.3.4/32'`. CIDR ranges
 * (e.g. `'10.0.0.0/24'`) are matched by the most-specific containing prefix.
 *
 * `requests_per_min = 0` blocks the IP entirely (kill switch).
 *
 * Lookup query (from middleware):
 *   SELECT requests_per_min FROM rate_limit_overrides
 *   WHERE ip_range >>= $1::inet
 *   ORDER BY masklen(ip_range) DESC LIMIT 1;
 */
export const rateLimitOverrides = pgTable(
	"rate_limit_overrides",
	{
		ipRange: cidr("ip_range").primaryKey(),
		requestsPerMin: integer("requests_per_min").notNull(),
		description: text(),
		createdAt: timestamp("created_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
		updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
	},
	(table) => [index("idx_rate_limit_overrides_ip_range").using("gist", table.ipRange.op("inet_ops"))],
)

/**
 * api_keys
 *
 * API keys for backend-to-backend rate limit identification. When a request
 * includes an `X-Api-Key` header matching an enabled row, the rate limiter
 * uses the key's `requests_per_min` instead of the IP-based default.
 *
 * `requests_per_min = NULL` means unlimited (no counter incremented).
 * `enabled = false` means the key is ignored (falls through to IP-based limiting).
 *
 * Keys are cached per-pod for RATE_LIMIT_OVERRIDE_CACHE_TTL_SECONDS (default 60s).
 */
export const apiKeys = pgTable("api_keys", {
	key: text().primaryKey(),
	clientName: text("client_name").notNull(),
	requestsPerMin: integer("requests_per_min"),
	enabled: boolean().notNull().default(true),
	createdAt: timestamp("created_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
	updatedAt: timestamp("updated_at", {withTimezone: true, mode: "date"}).defaultNow().notNull(),
})
