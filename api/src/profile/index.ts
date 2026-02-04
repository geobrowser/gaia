/**
 * Profile route handlers.
 *
 * Provides HTTP endpoints for fetching user profiles from the Knowledge Graph.
 * Profiles are derived from personal spaces - each user's wallet address maps
 * to a personal space whose entity contains profile data (name, avatar).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect, Either} from "effect"
import type {Context} from "hono"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"
import {isValidUuid} from "../utils/uuid"
import {
	defaultProfile,
	getProfileByAddress,
	getProfileBySpaceId,
	getProfilesBySpaceIds,
	type QueryError,
} from "./queries"
import type {Profile} from "./types"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

type Database = NodePgDatabase<Record<string, unknown>>

// Error type for profile validation failures
class ValidationError extends Data.TaggedError("ValidationError")<{
	message: string
}> {}

type ProfileError = ValidationError | QueryError

/**
 * Maximum number of space IDs for batch profile requests.
 */
const MAX_BATCH_SIZE = 100

/**
 * Validate an Ethereum address format (0x prefixed, 40 hex chars).
 */
function isValidAddress(address: string): boolean {
	return /^0x[a-fA-F0-9]{40}$/.test(address)
}

/**
 * Common OpenAPI error response schema.
 */
const errorResponseSchema = {
	type: "object" as const,
	properties: {
		error: {type: "string" as const},
		message: {type: "string" as const},
	},
}

/**
 * Map a ProfileError to an HTTP response.
 * Centralizes error handling to avoid duplication across routes.
 */
function handleProfileError(c: Context, error: ProfileError) {
	switch (error._tag) {
		case "ValidationError":
			return c.json({error: "Invalid parameter", message: error.message}, 400)
		case "QueryError":
			return c.json({error: "Internal server error", message: "An unexpected error occurred"}, 500)
	}
}

/**
 * Create the profile router.
 *
 * @param db - Drizzle database instance
 * @param runtime - Effect runtime with telemetry and other services
 * @returns Configured Hono router
 */
export function createProfileRouter(db: Database, runtime: AppRuntime) {
	const router = new Hono<AppEnv>()

	/**
	 * GET /profile/address/:address
	 *
	 * Get a user profile by wallet address.
	 */
	router.get(
		"/address/:address",
		describeRoute({
			tags: ["Profile"],
			summary: "Get profile by wallet address",
			description:
				"Returns a user profile for the given wallet address. The profile is derived from their personal space.",
			parameters: [
				{
					name: "address",
					in: "path",
					description: "Ethereum wallet address (0x prefixed)",
					required: true,
					schema: {type: "string", pattern: "^0x[a-fA-F0-9]{40}$"},
				},
			],
			responses: {
				200: {
					description: "Profile found",
					content: {"application/json": {schema: {$ref: "#/components/schemas/Profile"}}},
				},
				400: {
					description: "Invalid address format",
					content: {"application/json": {schema: errorResponseSchema}},
				},
				500: {
					description: "Internal server error",
					content: {"application/json": {schema: errorResponseSchema}},
				},
			},
		}),
		async (c) => {
			const address = c.req.param("address")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate address format
				if (!isValidAddress(address)) {
					yield* Effect.logWarning("Invalid address format", {address: address.slice(0, 10) + "..."})
					return yield* Effect.fail(
						new ValidationError({
							message: "Invalid Ethereum address format. Expected 0x-prefixed 40 hex characters.",
						}),
					)
				}

				// Normalize to lowercase for consistent lookups
				const normalizedAddress = address.toLowerCase()

				// Fetch profile
				const profile = yield* getProfileByAddress(db, normalizedAddress)

				const result = profile ?? defaultProfile(normalizedAddress)
				const found = profile !== null

				yield* Effect.logInfo("Profile fetched by address", {
					address: normalizedAddress,
					found,
					hasName: result.name !== null,
					hasAvatar: result.avatarUrl !== null,
				})

				return result
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError("Database error fetching profile by address", {
							operation: error.operation,
							cause: String(error.cause),
						})
					}
					return Effect.void
				}),
				Effect.withSpan("GET /profile/address/:address"),
				Effect.annotateSpans({requestId, address}),
				Effect.annotateLogs({requestId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error) => handleProfileError(c, error),
				onRight: (profile: Profile) => c.json(profile),
			})
		},
	)

	/**
	 * GET /profile/space/:spaceId
	 *
	 * Get a user profile by space ID.
	 */
	router.get(
		"/space/:spaceId",
		describeRoute({
			tags: ["Profile"],
			summary: "Get profile by space ID",
			description: "Returns a user profile for the given personal space ID.",
			parameters: [
				{
					name: "spaceId",
					in: "path",
					description: "Personal space UUID",
					required: true,
					schema: {type: "string", format: "uuid"},
				},
			],
			responses: {
				200: {
					description: "Profile found",
					content: {"application/json": {schema: {$ref: "#/components/schemas/Profile"}}},
				},
				400: {
					description: "Invalid space ID format",
					content: {"application/json": {schema: errorResponseSchema}},
				},
				500: {
					description: "Internal server error",
					content: {"application/json": {schema: errorResponseSchema}},
				},
			},
		}),
		async (c) => {
			const spaceId = c.req.param("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate space ID format (don't echo user input in error message)
				if (!isValidUuid(spaceId)) {
					yield* Effect.logWarning("Invalid space ID format")
					return yield* Effect.fail(new ValidationError({message: "Space ID must be a valid UUID"}))
				}

				// Fetch profile
				const profile = yield* getProfileBySpaceId(db, spaceId)

				const result = profile ?? defaultProfile(spaceId, spaceId)
				const found = profile !== null

				yield* Effect.logInfo("Profile fetched by space ID", {
					spaceId,
					found,
					hasName: result.name !== null,
					hasAvatar: result.avatarUrl !== null,
				})

				return result
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError("Database error fetching profile by space ID", {
							operation: error.operation,
							cause: String(error.cause),
						})
					}
					return Effect.void
				}),
				Effect.withSpan("GET /profile/space/:spaceId"),
				Effect.annotateSpans({requestId, spaceId}),
				Effect.annotateLogs({requestId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error) => handleProfileError(c, error),
				onRight: (profile: Profile) => c.json(profile),
			})
		},
	)

	/**
	 * POST /profile/batch
	 *
	 * Batch fetch profiles by space IDs.
	 */
	router.post(
		"/batch",
		describeRoute({
			tags: ["Profile"],
			summary: "Batch fetch profiles by space IDs",
			description: `Fetches multiple profiles in a single request. Returns profiles in the same order as the input array. Maximum ${MAX_BATCH_SIZE} space IDs per request.`,
			requestBody: {
				required: true,
				content: {
					"application/json": {
						schema: {
							type: "object",
							properties: {
								spaceIds: {
									type: "array",
									items: {type: "string", format: "uuid"},
									maxItems: MAX_BATCH_SIZE,
									description: "Array of space UUIDs to fetch profiles for",
								},
							},
							required: ["spaceIds"],
						},
					},
				},
			},
			responses: {
				200: {
					description: "Profiles fetched successfully",
					content: {
						"application/json": {
							schema: {
								type: "object",
								properties: {
									profiles: {
										type: "array",
										items: {$ref: "#/components/schemas/Profile"},
									},
								},
								required: ["profiles"],
							},
						},
					},
				},
				400: {
					description: "Invalid request",
					content: {"application/json": {schema: errorResponseSchema}},
				},
				500: {
					description: "Internal server error",
					content: {"application/json": {schema: errorResponseSchema}},
				},
			},
		}),
		async (c) => {
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Parse request body
				const body = yield* Effect.tryPromise({
					try: () => c.req.json<{spaceIds?: unknown}>(),
					catch: () => new ValidationError({message: "Invalid JSON body"}),
				})

				// Validate spaceIds array
				if (!body.spaceIds || !Array.isArray(body.spaceIds)) {
					yield* Effect.logWarning("Invalid batch request: spaceIds must be an array")
					return yield* Effect.fail(new ValidationError({message: "spaceIds must be an array"}))
				}

				const spaceIds = body.spaceIds as unknown[]
				const batchSize = spaceIds.length

				// Handle empty array
				if (batchSize === 0) {
					yield* Effect.logInfo("Batch profile fetch completed", {batchSize: 0, found: 0})
					return {profiles: []}
				}

				// Check batch size limit
				if (batchSize > MAX_BATCH_SIZE) {
					yield* Effect.logWarning("Batch size exceeded limit", {batchSize, maxBatchSize: MAX_BATCH_SIZE})
					return yield* Effect.fail(
						new ValidationError({message: `Maximum ${MAX_BATCH_SIZE} space IDs allowed per request`}),
					)
				}

				// Validate each space ID (don't echo user input in error message)
				const invalidIndex = spaceIds.findIndex((id) => typeof id !== "string" || !isValidUuid(id))
				if (invalidIndex !== -1) {
					yield* Effect.logWarning("Invalid space ID in batch", {index: invalidIndex})
					return yield* Effect.fail(new ValidationError({message: "Invalid space ID format in request"}))
				}

				const validSpaceIds = spaceIds as string[]

				// Fetch profiles
				const profileMap = yield* getProfilesBySpaceIds(db, validSpaceIds)

				// Return profiles in the same order as input, with defaults for missing
				const profiles = validSpaceIds.map((id) => profileMap.get(id) ?? defaultProfile(id, id))
				const foundCount = profileMap.size

				yield* Effect.logInfo("Batch profile fetch completed", {
					batchSize,
					found: foundCount,
					missing: batchSize - foundCount,
				})

				return {profiles}
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError("Database error in batch profile fetch", {
							operation: error.operation,
							cause: String(error.cause),
						})
					}
					return Effect.void
				}),
				Effect.withSpan("POST /profile/batch"),
				Effect.annotateSpans({requestId}),
				Effect.annotateLogs({requestId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error) => handleProfileError(c, error),
				onRight: (response) => c.json(response),
			})
		},
	)

	return router
}

export type {Profile} from "./types"
