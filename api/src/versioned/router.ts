/**
 * Versioned entities router.
 *
 * Provides REST endpoints for querying entity state at specific versions
 * and computing diffs between versions.
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

import {isValidUuid} from "../utils/uuid"
import {diffGroupedEntitySnapshots} from "./diff"
import {
	computeProposalDiff,
	EditBlobNotCachedError,
	EditDecodeError,
	ProposalNotFoundError,
} from "./proposal-diff"
import {
	getEntitySnapshotAtVersion,
	getEntityVersions,
	getGroupedEntitySnapshotAtVersion,
	type QueryError,
	resolveVersionKey,
} from "./queries"
import type {EntitySnapshot, GroupedEntityDiff, PaginatedProposalDiff, VersionEntry} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

// Error types for versioned operations
class ValidationError extends Data.TaggedError("ValidationError")<{
	message: string
}> {}

class NotFoundError extends Data.TaggedError("NotFoundError")<{
	message: string
}> {}

type VersionedError = ValidationError | NotFoundError | QueryError

type ProposalError =
	| ValidationError
	| NotFoundError
	| QueryError
	| ProposalNotFoundError
	| EditBlobNotCachedError
	| EditDecodeError

/**
 * Create the versioned entities router.
 *
 * @param db - Drizzle database instance
 * @param runtime - Effect runtime with telemetry and other services
 * @returns Configured Hono router
 */
export function createVersionedRouter(db: Database, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	/**
	 * GET /versioned/entities/:id
	 *
	 * Get an entity snapshot at a specific version.
	 */
	router.get(
		"/entities/:id",
		describeRoute({
			tags: ["Versioned Entities"],
			summary: "Get entity snapshot at version",
			description: "Returns the state of an entity at a specific version (edit)",
			parameters: [
				{
					name: "id",
					in: "path",
					description: "Entity UUID",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "editId",
					in: "query",
					description: "Edit UUID to retrieve entity state at",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "spaceId",
					in: "query",
					description: "Space UUID to scope the query to",
					required: false,
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "Entity snapshot",
					content: {
						"application/json": {
							schema: {
								$ref: "#/components/schemas/EntitySnapshot",
							},
						},
					},
				},
				400: {
					description: "Invalid parameter",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				404: {
					description: "Entity or edit not found",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				500: {
					description: "Internal server error",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const entityId = c.req.param("id")
			const editId = c.req.query("editId")
			const spaceId = c.req.query("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(entityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate editId is provided
				if (!editId) {
					return yield* Effect.fail(new ValidationError({message: "editId query parameter is required"}))
				}

				if (!isValidUuid(editId)) {
					return yield* Effect.fail(new ValidationError({message: "editId must be a valid UUID"}))
				}

				// Validate spaceId if provided
				if (spaceId && !isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				// Resolve edit to version key
				const versionKey = yield* resolveVersionKey(db, editId)

				if (versionKey === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${editId}' not found`}))
				}

				// Get entity snapshot at version
				const snapshot = yield* getEntitySnapshotAtVersion(db, entityId, versionKey, spaceId)

				return snapshot
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(
							`Database error: operation=${error.operation}, cause=${String(error.cause)}`,
						)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /versioned/entities/:id"),
				Effect.annotateSpans({requestId, entityId, editId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: VersionedError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json(
								{error: "Internal server error", message: "An unexpected error occurred"},
								500,
							)
					}
				},
				onRight: (snapshot: EntitySnapshot) => c.json(snapshot),
			})
		},
	)

	/**
	 * GET /versioned/entities/:id/versions
	 *
	 * List versions (edits) that affected an entity.
	 */
	router.get(
		"/entities/:id/versions",
		describeRoute({
			tags: ["Versioned Entities"],
			summary: "List entity versions",
			description: "Returns a list of edits that affected an entity",
			parameters: [
				{
					name: "id",
					in: "path",
					description: "Entity UUID",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "spaceId",
					in: "query",
					description: "Space UUID to scope the query to",
					required: false,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "limit",
					in: "query",
					description: "Maximum number of versions to return",
					required: false,
					schema: {type: "integer", minimum: 1, maximum: 100, default: 50},
				},
				{
					name: "offset",
					in: "query",
					description: "Number of versions to skip for pagination",
					required: false,
					schema: {type: "integer", minimum: 0, default: 0},
				},
			],
			responses: {
				200: {
					description: "List of versions",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									versions: {
										type: "array",
										items: {
											$ref: "#/components/schemas/VersionEntry",
										},
									},
								},
								required: ["versions"],
							},
						},
					},
				},
				400: {
					description: "Invalid parameter",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				500: {
					description: "Internal server error",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const entityId = c.req.param("id")
			const spaceId = c.req.query("spaceId")
			const limitParam = c.req.query("limit")
			const offsetParam = c.req.query("offset")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(entityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate spaceId if provided
				if (spaceId && !isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				// Parse and validate limit
				let limit = 50
				if (limitParam) {
					const parsed = parseInt(limitParam, 10)
					if (Number.isNaN(parsed) || parsed < 1) {
						return yield* Effect.fail(new ValidationError({message: "limit must be a positive integer"}))
					}
					limit = Math.min(parsed, 100)
				}

				// Parse and validate offset
				let offset = 0
				if (offsetParam) {
					const parsed = parseInt(offsetParam, 10)
					if (Number.isNaN(parsed) || parsed < 0) {
						return yield* Effect.fail(
							new ValidationError({message: "offset must be a non-negative integer"}),
						)
					}
					offset = parsed
				}

				// Get entity versions
				const versions = yield* getEntityVersions(db, entityId, spaceId, limit, offset)

				return versions
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(
							`Database error: operation=${error.operation}, cause=${String(error.cause)}`,
						)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /versioned/entities/:id/versions"),
				Effect.annotateSpans({requestId, entityId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: VersionedError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json(
								{error: "Internal server error", message: "An unexpected error occurred"},
								500,
							)
					}
				},
				onRight: (versions: VersionEntry[]) => c.json({versions}),
			})
		},
	)

	/**
	 * GET /versioned/entities/:id/diff
	 *
	 * Compute a diff between two versions of an entity.
	 */
	router.get(
		"/entities/:id/diff",
		describeRoute({
			tags: ["Versioned Entities"],
			summary: "Compute entity diff between versions",
			description: "Computes the difference between two versions of an entity",
			parameters: [
				{
					name: "id",
					in: "path",
					description: "Entity UUID",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "fromEditId",
					in: "query",
					description: "Starting edit UUID for the diff",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "toEditId",
					in: "query",
					description: "Ending edit UUID for the diff",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "spaceId",
					in: "query",
					description: "Space UUID (required for diffs)",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "Entity diff with dynamic group keys spread at root level",
					content: {
						"application/json": {
							schema: {
								$ref: "#/components/schemas/GroupedEntityDiffResponse",
							},
						},
					},
				},
				400: {
					description: "Invalid parameter",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				404: {
					description: "Entity or edit not found",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				500: {
					description: "Internal server error",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const entityId = c.req.param("id")
			const fromEditId = c.req.query("fromEditId")
			const toEditId = c.req.query("toEditId")
			const spaceId = c.req.query("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(entityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate required parameters
				if (!fromEditId) {
					return yield* Effect.fail(new ValidationError({message: "fromEditId query parameter is required"}))
				}

				if (!toEditId) {
					return yield* Effect.fail(new ValidationError({message: "toEditId query parameter is required"}))
				}

				if (!spaceId) {
					return yield* Effect.fail(
						new ValidationError({message: "spaceId query parameter is required for diffs"}),
					)
				}

				// Validate UUIDs
				if (!isValidUuid(fromEditId)) {
					return yield* Effect.fail(new ValidationError({message: "fromEditId must be a valid UUID"}))
				}

				if (!isValidUuid(toEditId)) {
					return yield* Effect.fail(new ValidationError({message: "toEditId must be a valid UUID"}))
				}

				if (!isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				// Resolve both edits to version keys
				const [fromVersionKey, toVersionKey] = yield* Effect.all([
					resolveVersionKey(db, fromEditId),
					resolveVersionKey(db, toEditId),
				])

				if (fromVersionKey === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${fromEditId}' not found`}))
				}

				if (toVersionKey === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${toEditId}' not found`}))
				}

				// Get grouped snapshots at both versions
				const [fromSnapshot, toSnapshot] = yield* Effect.all([
					getGroupedEntitySnapshotAtVersion(db, entityId, fromVersionKey, spaceId),
					getGroupedEntitySnapshotAtVersion(db, entityId, toVersionKey, spaceId),
				])

				// Compute grouped diff
				const diff = yield* diffGroupedEntitySnapshots(entityId, fromSnapshot, toSnapshot)

				return diff
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(
							`Database error: operation=${error.operation}, cause=${String(error.cause)}`,
						)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /versioned/entities/:id/diff"),
				Effect.annotateSpans({requestId, entityId, fromEditId, toEditId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: VersionedError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json(
								{error: "Internal server error", message: "An unexpected error occurred"},
								500,
							)
					}
				},
				onRight: (diff: GroupedEntityDiff) => {
					// Spread dynamic groups at root level per spec
					const {groups, ...rest} = diff
					return c.json({...rest, ...groups})
				},
			})
		},
	)

	/**
	 * GET /versioned/proposals/:id/diff
	 *
	 * Compute a diff between a proposal's proposed changes and the base state.
	 * - Active proposals: compare against current live state
	 * - Closed proposals: compare against versioned state at end_time
	 */
	router.get(
		"/proposals/:id/diff",
		describeRoute({
			tags: ["Versioned Entities"],
			summary: "Compute proposal diff",
			description:
				"Computes the difference between a proposal's proposed changes and the base state. For active proposals, compares against current live state. For closed proposals, compares against versioned state at end_time.",
			parameters: [
				{
					name: "id",
					in: "path",
					description: "Proposal UUID",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "spaceId",
					in: "query",
					description: "Space UUID to scope the diff",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
				{
					name: "cursor",
					in: "query",
					description: "Pagination cursor for fetching next page",
					required: false,
					schema: {type: "string"},
				},
				{
					name: "limit",
					in: "query",
					description: "Maximum number of entities per page",
					required: false,
					schema: {type: "integer", minimum: 1, maximum: 100, default: 50},
				},
			],
			responses: {
				200: {
					description: "Paginated proposal diff",
					content: {
						"application/json": {
							schema: {
								$ref: "#/components/schemas/PaginatedProposalDiff",
							},
						},
					},
				},
				400: {
					description: "Invalid parameter",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				404: {
					description: "Proposal not found or edit blob not cached",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
				500: {
					description: "Internal server error",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									error: {type: "string"},
									message: {type: "string"},
								},
							},
						},
					},
				},
			},
		}),
		async (c) => {
			const proposalId = c.req.param("id")
			const spaceId = c.req.query("spaceId")
			const cursor = c.req.query("cursor")
			const limitParam = c.req.query("limit")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate proposalId
				if (!isValidUuid(proposalId)) {
					return yield* Effect.fail(new ValidationError({message: "Proposal ID must be a valid UUID"}))
				}

				// Validate spaceId is provided
				if (!spaceId) {
					return yield* Effect.fail(
						new ValidationError({message: "spaceId query parameter is required"}),
					)
				}

				if (!isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				// Parse and validate limit
				let limit = 50
				if (limitParam) {
					const parsed = parseInt(limitParam, 10)
					if (Number.isNaN(parsed) || parsed < 1) {
						return yield* Effect.fail(new ValidationError({message: "limit must be a positive integer"}))
					}
					limit = Math.min(parsed, 100)
				}

				// Compute proposal diff
				const diff = yield* computeProposalDiff(db, proposalId, spaceId, cursor, limit)

				return diff
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(
							`Database error: operation=${error.operation}, cause=${String(error.cause)}`,
						)
					}
					if (error._tag === "EditDecodeError") {
						return Effect.logError(`Edit decode error: ${String(error.cause)}`)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /versioned/proposals/:id/diff"),
				Effect.annotateSpans({requestId, proposalId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProposalError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "ProposalNotFoundError":
							return c.json({error: "Not found", message: `Proposal '${error.proposalId}' not found`}, 404)
						case "EditBlobNotCachedError":
							return c.json(
								{error: "Not found", message: `Edit blob not cached for URI: ${error.uri}`},
								404,
							)
						case "EditDecodeError":
							return c.json(
								{error: "Internal server error", message: "Failed to decode edit blob"},
								500,
							)
						case "QueryError":
							return c.json(
								{error: "Internal server error", message: "An unexpected error occurred"},
								500,
							)
					}
				},
				onRight: (diff: PaginatedProposalDiff) => c.json(diff),
			})
		},
	)

	return router
}
