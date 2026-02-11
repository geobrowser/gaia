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

import type {Profile} from "../profile/types"
import type {AppRuntime} from "../services/runtime"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

import {getProfilesByEntityIds} from "../profile/queries"
import {isValidUuid, normalizeUuid, toDashedUuid} from "../utils/uuid"
import {diffGroupedEntitySnapshots} from "./diff"
import type {
	EditBlobNotCachedError,
	EditDecodeError,
	InvalidCursorError,
	ProposalNotFoundError,
	SpaceMismatchError,
} from "./proposal-diff"
import {computeProposalDiff} from "./proposal-diff"
import {
	getEntitySnapshotAtVersion,
	getEntityVersions,
	getGroupedEntitySnapshotAtVersion,
	type QueryError,
	resolveVersionKey,
} from "./queries"
import type {DiffResponse, PaginatedProposalDiff, SnapshotResponse, VersionEntry} from "./types"

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
	| SpaceMismatchError
	| InvalidCursorError

/**
 * Batch-resolve creator profiles from a list of nullable Person Entity IDs.
 * Deduplicates IDs, fetches profiles, and returns a lookup map.
 * Degrades gracefully on failure (logs a warning, returns empty map).
 */
function resolveCreatorProfiles(
	db: Database,
	creatorIds: (string | null)[],
): Effect.Effect<Map<string, Profile>, never> {
	const unique = [...new Set(creatorIds.filter((id): id is string => id !== null))]
	if (unique.length === 0) return Effect.succeed(new Map())
	return getProfilesByEntityIds(db, unique.map(toDashedUuid)).pipe(
		Effect.tapError((err) =>
			Effect.logWarning("Profile resolution failed, degrading gracefully", {
				cause: String(err),
			}),
		),
		Effect.catchAll(() => Effect.succeed(new Map())),
	)
}

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
			const rawEntityId = c.req.param("id")
			const rawEditId = c.req.query("editId")
			const rawSpaceId = c.req.query("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(rawEntityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate editId is provided
				if (!rawEditId) {
					return yield* Effect.fail(
						new ValidationError({
							message: "editId query parameter is required",
						}),
					)
				}

				if (!isValidUuid(rawEditId)) {
					return yield* Effect.fail(new ValidationError({message: "editId must be a valid UUID"}))
				}

				// Validate spaceId if provided
				if (rawSpaceId && !isValidUuid(rawSpaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				const entityId = normalizeUuid(rawEntityId)
				const editId = normalizeUuid(rawEditId)
				const spaceId = rawSpaceId ? normalizeUuid(rawSpaceId) : undefined

				// Resolve edit to version key and name
				const resolved = yield* resolveVersionKey(db, editId)

				if (resolved === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${editId}' not found`}))
				}

				// Get entity snapshot at version
				const snapshot = yield* getEntitySnapshotAtVersion(db, entityId, resolved.versionKey, spaceId)

				// Resolve creator profile
				const profileMap = yield* resolveCreatorProfiles(db, [resolved.createdById])
				const createdBy = resolved.createdById ? (profileMap.get(resolved.createdById) ?? null) : null

				return {
					editName: resolved.name,
					createdById: resolved.createdById,
					createdBy,
					...snapshot,
				} satisfies SnapshotResponse
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
				Effect.annotateSpans({
					requestId,
					entityId: rawEntityId,
					editId: rawEditId,
					spaceId: rawSpaceId,
				}),
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
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
					}
				},
				onRight: (snapshot) => c.json(snapshot),
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
			const rawEntityId = c.req.param("id")
			const rawSpaceId = c.req.query("spaceId")
			const limitParam = c.req.query("limit")
			const offsetParam = c.req.query("offset")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(rawEntityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate spaceId if provided
				if (rawSpaceId && !isValidUuid(rawSpaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				const entityId = normalizeUuid(rawEntityId)
				const spaceId = rawSpaceId ? normalizeUuid(rawSpaceId) : undefined

				// Parse and validate limit
				let limit = 50
				if (limitParam) {
					const parsed = parseInt(limitParam, 10)
					if (Number.isNaN(parsed) || parsed < 1) {
						return yield* Effect.fail(
							new ValidationError({
								message: "limit must be a positive integer",
							}),
						)
					}
					limit = Math.min(parsed, 100)
				}

				// Parse and validate offset
				let offset = 0
				if (offsetParam) {
					const parsed = parseInt(offsetParam, 10)
					if (Number.isNaN(parsed) || parsed < 0) {
						return yield* Effect.fail(
							new ValidationError({
								message: "offset must be a non-negative integer",
							}),
						)
					}
					offset = parsed
				}

				// Get entity versions
				const versions = yield* getEntityVersions(db, entityId, spaceId, limit, offset)

				// Batch-resolve creator profiles server-side to avoid client N+1
				const profileMap = yield* resolveCreatorProfiles(
					db,
					versions.map((v) => v.createdById),
				)

				return versions.map((v) => ({
					...v,
					createdBy: v.createdById ? (profileMap.get(v.createdById) ?? null) : null,
				}))
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
				Effect.annotateSpans({
					requestId,
					entityId: rawEntityId,
					spaceId: rawSpaceId,
				}),
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
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
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
			const rawEntityId = c.req.param("id")
			const rawFromEditId = c.req.query("fromEditId")
			const rawToEditId = c.req.query("toEditId")
			const rawSpaceId = c.req.query("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate entityId
				if (!isValidUuid(rawEntityId)) {
					return yield* Effect.fail(new ValidationError({message: "Entity ID must be a valid UUID"}))
				}

				// Validate required parameters
				if (!rawFromEditId) {
					return yield* Effect.fail(
						new ValidationError({
							message: "fromEditId query parameter is required",
						}),
					)
				}

				if (!rawToEditId) {
					return yield* Effect.fail(
						new ValidationError({
							message: "toEditId query parameter is required",
						}),
					)
				}

				if (!rawSpaceId) {
					return yield* Effect.fail(
						new ValidationError({
							message: "spaceId query parameter is required for diffs",
						}),
					)
				}

				// Validate UUIDs
				if (!isValidUuid(rawFromEditId)) {
					return yield* Effect.fail(new ValidationError({message: "fromEditId must be a valid UUID"}))
				}

				if (!isValidUuid(rawToEditId)) {
					return yield* Effect.fail(new ValidationError({message: "toEditId must be a valid UUID"}))
				}

				if (!isValidUuid(rawSpaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				const entityId = normalizeUuid(rawEntityId)
				const fromEditId = normalizeUuid(rawFromEditId)
				const toEditId = normalizeUuid(rawToEditId)
				const spaceId = normalizeUuid(rawSpaceId)

				// Resolve both edits to version keys and names
				const [fromResolved, toResolved] = yield* Effect.all([
					resolveVersionKey(db, fromEditId),
					resolveVersionKey(db, toEditId),
				])

				if (fromResolved === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${fromEditId}' not found`}))
				}

				if (toResolved === null) {
					return yield* Effect.fail(new NotFoundError({message: `Edit '${toEditId}' not found`}))
				}

				// Get grouped snapshots at both versions
				const [fromSnapshot, toSnapshot] = yield* Effect.all([
					getGroupedEntitySnapshotAtVersion(db, entityId, fromResolved.versionKey, spaceId),
					getGroupedEntitySnapshotAtVersion(db, entityId, toResolved.versionKey, spaceId),
				])

				// Compute grouped diff
				const diff = yield* diffGroupedEntitySnapshots(entityId, fromSnapshot, toSnapshot)

				// Resolve creator profiles for both edits
				const profileMap = yield* resolveCreatorProfiles(db, [fromResolved.createdById, toResolved.createdById])

				return {
					...diff,
					fromEditName: fromResolved.name,
					fromCreatedById: fromResolved.createdById,
					fromCreatedBy: fromResolved.createdById ? (profileMap.get(fromResolved.createdById) ?? null) : null,
					toEditName: toResolved.name,
					toCreatedById: toResolved.createdById,
					toCreatedBy: toResolved.createdById ? (profileMap.get(toResolved.createdById) ?? null) : null,
				} as const
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
				Effect.annotateSpans({
					requestId,
					entityId: rawEntityId,
					fromEditId: rawFromEditId,
					toEditId: rawToEditId,
					spaceId: rawSpaceId,
				}),
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
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
					}
				},
				onRight: (diff) => {
					// Spread dynamic groups at root level per spec
					const {groups, ...rest} = diff
					return c.json({...rest, ...groups} as DiffResponse)
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
			const rawProposalId = c.req.param("id")
			const rawSpaceId = c.req.query("spaceId")
			const cursor = c.req.query("cursor")
			const limitParam = c.req.query("limit")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate proposalId
				if (!isValidUuid(rawProposalId)) {
					return yield* Effect.fail(
						new ValidationError({
							message: "Proposal ID must be a valid UUID",
						}),
					)
				}

				// Validate spaceId is provided
				if (!rawSpaceId) {
					return yield* Effect.fail(
						new ValidationError({
							message: "spaceId query parameter is required",
						}),
					)
				}

				if (!isValidUuid(rawSpaceId)) {
					return yield* Effect.fail(new ValidationError({message: "spaceId must be a valid UUID"}))
				}

				const proposalId = normalizeUuid(rawProposalId)
				const spaceId = normalizeUuid(rawSpaceId)

				// Parse and validate limit
				let limit = 50
				if (limitParam) {
					const parsed = parseInt(limitParam, 10)
					if (Number.isNaN(parsed) || parsed < 1) {
						return yield* Effect.fail(
							new ValidationError({
								message: "limit must be a positive integer",
							}),
						)
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
				Effect.annotateSpans({
					requestId,
					proposalId: rawProposalId,
					spaceId: rawSpaceId,
					cursor,
					limit: limitParam,
				}),
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
							return c.json({error: "Not found", message: "Proposal not found"}, 404)
						case "EditBlobNotCachedError":
							return c.json(
								{
									error: "Not found",
									message: "Edit blob not cached for this proposal",
								},
								404,
							)
						case "SpaceMismatchError":
							return c.json(
								{
									error: "Invalid parameter",
									message: "spaceId does not match the proposal's space",
								},
								400,
							)
						case "InvalidCursorError":
							return c.json(
								{
									error: "Invalid parameter",
									message: "Invalid pagination cursor",
								},
								400,
							)
						case "EditDecodeError":
							return c.json(
								{
									error: "Internal server error",
									message: "Failed to decode edit blob",
								},
								500,
							)
						case "QueryError":
							return c.json(
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
						default: {
							// Exhaustive check - TypeScript will error if a case is missing
							const _exhaustive: never = error
							return c.json(
								{
									error: "Internal server error",
									message: "An unexpected error occurred",
								},
								500,
							)
						}
					}
				},
				onRight: (diff: PaginatedProposalDiff) => c.json(diff),
			})
		},
	)

	return router
}
