/**
 * Profile route handlers.
 *
 * Provides HTTP endpoints for fetching user profiles from the Knowledge Graph.
 * Profiles are derived from personal spaces - each user's wallet address maps
 * to a personal space whose entity contains profile data (name, avatar, cover).
 */

import type {NodePgDatabase} from "drizzle-orm/node-postgres"
import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import {describeRoute} from "hono-openapi"

import type {AppRuntime} from "../services/runtime"
import {isValidUuid} from "../utils/uuid"
import {defaultProfile, getProfileByAddress, getProfileBySpaceId, getProfilesBySpaceIds, QueryError} from "./queries"
import type {Profile} from "./types"

type AppEnv = {
	Variables: {
		requestId: string
	}
}

type Database = NodePgDatabase<Record<string, unknown>>

// Error types for profile operations
class ValidationError extends Data.TaggedError("ValidationError")<{
	message: string
}> {}

class NotFoundError extends Data.TaggedError("NotFoundError")<{
	message: string
}> {}

type ProfileError = ValidationError | NotFoundError | QueryError

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
			description: "Returns a user profile for the given wallet address. The profile is derived from their personal space.",
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
					content: {
						"application/json": {
							schema: {
								$ref: "#/components/schemas/Profile",
							},
						},
					},
				},
				400: {
					description: "Invalid address format",
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
					description: "No profile found for this address",
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
			const address = c.req.param("address")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate address format
				if (!isValidAddress(address)) {
					return yield* Effect.fail(
						new ValidationError({message: "Invalid Ethereum address format. Expected 0x-prefixed 40 hex characters."}),
					)
				}

				// Normalize to lowercase for consistent lookups
				const normalizedAddress = address.toLowerCase()

				// Fetch profile
				const profile = yield* getProfileByAddress(db, normalizedAddress)

				if (!profile) {
					// Return a default profile instead of 404 for better UX
					return defaultProfile(normalizedAddress)
				}

				return profile
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(`Database error: operation=${error.operation}, cause=${String(error.cause)}`)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /profile/address/:address"),
				Effect.annotateSpans({requestId, address}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProfileError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json({error: "Internal server error", message: "An unexpected error occurred"}, 500)
					}
				},
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
					content: {
						"application/json": {
							schema: {
								$ref: "#/components/schemas/Profile",
							},
						},
					},
				},
				400: {
					description: "Invalid space ID format",
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
					description: "Space not found",
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
			const spaceId = c.req.param("spaceId")
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Validate space ID format
				if (!isValidUuid(spaceId)) {
					return yield* Effect.fail(new ValidationError({message: "Space ID must be a valid UUID"}))
				}

				// Fetch profile
				const profile = yield* getProfileBySpaceId(db, spaceId)

				if (!profile) {
					// Return a default profile instead of 404 for better UX
					return defaultProfile(spaceId, spaceId)
				}

				return profile
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(`Database error: operation=${error.operation}, cause=${String(error.cause)}`)
					}
					return Effect.void
				}),
				Effect.withSpan("GET /profile/space/:spaceId"),
				Effect.annotateSpans({requestId, spaceId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProfileError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid parameter", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json({error: "Internal server error", message: "An unexpected error occurred"}, 500)
					}
				},
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
			const requestId = c.get("requestId") ?? "unknown"

			const program = Effect.gen(function* () {
				// Parse request body
				const body = yield* Effect.tryPromise({
					try: () => c.req.json<{spaceIds?: unknown}>(),
					catch: () => new ValidationError({message: "Invalid JSON body"}),
				})

				// Validate spaceIds
				if (!body.spaceIds || !Array.isArray(body.spaceIds)) {
					return yield* Effect.fail(new ValidationError({message: "spaceIds must be an array"}))
				}

				const spaceIds = body.spaceIds as unknown[]

				if (spaceIds.length === 0) {
					return {profiles: []}
				}

				if (spaceIds.length > MAX_BATCH_SIZE) {
					return yield* Effect.fail(
						new ValidationError({message: `Maximum ${MAX_BATCH_SIZE} space IDs allowed per request`}),
					)
				}

				// Validate each space ID
				for (const id of spaceIds) {
					if (typeof id !== "string" || !isValidUuid(id)) {
						return yield* Effect.fail(new ValidationError({message: `Invalid space ID: ${id}`}))
					}
				}

				const validSpaceIds = spaceIds as string[]

				// Fetch profiles
				const profileMap = yield* getProfilesBySpaceIds(db, validSpaceIds)

				// Return profiles in the same order as input, with defaults for missing
				const profiles = validSpaceIds.map((id) => profileMap.get(id) ?? defaultProfile(id, id))

				return {profiles}
			}).pipe(
				Effect.tapError((error) => {
					if (error._tag === "QueryError") {
						return Effect.logError(`Database error: operation=${error.operation}, cause=${String(error.cause)}`)
					}
					return Effect.void
				}),
				Effect.withSpan("POST /profile/batch"),
				Effect.annotateSpans({requestId}),
			)

			const result = await runtime.runPromise(Effect.either(program))

			return Either.match(result, {
				onLeft: (error: ProfileError) => {
					switch (error._tag) {
						case "ValidationError":
							return c.json({error: "Invalid request", message: error.message}, 400)
						case "NotFoundError":
							return c.json({error: "Not found", message: error.message}, 404)
						case "QueryError":
							return c.json({error: "Internal server error", message: "An unexpected error occurred"}, 500)
					}
				},
				onRight: (response) => c.json(response),
			})
		},
	)

	return router
}

export {type Profile} from "./types"
