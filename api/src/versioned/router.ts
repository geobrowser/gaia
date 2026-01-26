/**
 * Versioned entities router.
 *
 * Provides REST endpoints for querying entity state at specific versions
 * and computing diffs between versions.
 */

import {Data, Effect, Either} from "effect"
import {Hono} from "hono"
import type {NodePgDatabase} from "drizzle-orm/node-postgres"

import type {AppRuntime} from "../services/runtime"
import {isValidUuid} from "../utils/uuid"
import {resolveVersionKey, getEntitySnapshotAtVersion, getEntityVersions} from "./queries"
import {diffEntitySnapshots} from "./diff"
import type {EntityDiff, EntitySnapshot, VersionEntry} from "./types"

type Database = NodePgDatabase<Record<string, unknown>>

// Error types for versioned operations
class ValidationError extends Data.TaggedError("ValidationError")<{
	message: string
}> {}

class NotFoundError extends Data.TaggedError("NotFoundError")<{
	message: string
}> {}

class InternalError extends Data.TaggedError("InternalError")<{
	message: string
}> {}

type VersionedError = ValidationError | NotFoundError | InternalError

/**
 * Create the versioned entities router.
 *
 * @param db - Drizzle database instance
 * @param runtime - Effect runtime with telemetry and other services
 * @returns Configured Hono router
 */
export function createVersionedRouter(db: Database, runtime: AppRuntime) {
	const router = new Hono()

	/**
	 * GET /versioned/entities/:id
	 *
	 * Get an entity snapshot at a specific version.
	 */
	router.get("/entities/:id", async (c) => {
		const entityId = c.req.param("id")
		const editId = c.req.query("editId")
		const spaceId = c.req.query("spaceId")

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
			const versionKey = yield* Effect.tryPromise({
				try: () => resolveVersionKey(db, editId),
				catch: (error) => new InternalError({message: String(error)}),
			}).pipe(Effect.withSpan("resolveVersionKey"))

			if (versionKey === null) {
				return yield* Effect.fail(new NotFoundError({message: `Edit '${editId}' not found`}))
			}

			// Get entity snapshot at version
			const snapshot = yield* Effect.tryPromise({
				try: () => getEntitySnapshotAtVersion(db, entityId, versionKey, spaceId),
				catch: (error) => new InternalError({message: String(error)}),
			}).pipe(Effect.withSpan("getEntitySnapshotAtVersion"))

			return snapshot
		}).pipe(
			Effect.withSpan("GET /versioned/entities/:id"),
			Effect.annotateLogs({entityId, editId}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error: VersionedError) => {
				switch (error._tag) {
					case "ValidationError":
						return c.json({error: "Invalid parameter", message: error.message}, 400)
					case "NotFoundError":
						return c.json({error: "Not found", message: error.message}, 404)
					case "InternalError":
						return c.json({error: "Internal error", message: error.message}, 500)
				}
			},
			onRight: (snapshot: EntitySnapshot) => c.json(snapshot),
		})
	})

	/**
	 * GET /versioned/entities/:id/versions
	 *
	 * List versions (edits) that affected an entity.
	 */
	router.get("/entities/:id/versions", async (c) => {
		const entityId = c.req.param("id")
		const spaceId = c.req.query("spaceId")
		const limitParam = c.req.query("limit")
		const offsetParam = c.req.query("offset")

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
					return yield* Effect.fail(new ValidationError({message: "offset must be a non-negative integer"}))
				}
				offset = parsed
			}

			// Get entity versions
			const versions = yield* Effect.tryPromise({
				try: () => getEntityVersions(db, entityId, spaceId, limit, offset),
				catch: (error) => new InternalError({message: String(error)}),
			}).pipe(Effect.withSpan("getEntityVersions"))

			return versions
		}).pipe(
			Effect.withSpan("GET /versioned/entities/:id/versions"),
			Effect.annotateLogs({entityId}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error: VersionedError) => {
				switch (error._tag) {
					case "ValidationError":
						return c.json({error: "Invalid parameter", message: error.message}, 400)
					case "NotFoundError":
						return c.json({error: "Not found", message: error.message}, 404)
					case "InternalError":
						return c.json({error: "Internal error", message: error.message}, 500)
				}
			},
			onRight: (versions: VersionEntry[]) => c.json({versions}),
		})
	})

	/**
	 * GET /versioned/entities/:id/diff
	 *
	 * Compute a diff between two versions of an entity.
	 */
	router.get("/entities/:id/diff", async (c) => {
		const entityId = c.req.param("id")
		const fromEditId = c.req.query("fromEditId")
		const toEditId = c.req.query("toEditId")
		const spaceId = c.req.query("spaceId")

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
				return yield* Effect.fail(new ValidationError({message: "spaceId query parameter is required for diffs"}))
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
			const [fromVersionKey, toVersionKey] = yield* Effect.tryPromise({
				try: () => Promise.all([resolveVersionKey(db, fromEditId), resolveVersionKey(db, toEditId)]),
				catch: (error) => new InternalError({message: String(error)}),
			}).pipe(Effect.withSpan("resolveVersionKeys"))

			if (fromVersionKey === null) {
				return yield* Effect.fail(new NotFoundError({message: `Edit '${fromEditId}' not found`}))
			}

			if (toVersionKey === null) {
				return yield* Effect.fail(new NotFoundError({message: `Edit '${toEditId}' not found`}))
			}

			// Get snapshots at both versions
			const [fromSnapshot, toSnapshot] = yield* Effect.tryPromise({
				try: () =>
					Promise.all([
						getEntitySnapshotAtVersion(db, entityId, fromVersionKey, spaceId),
						getEntitySnapshotAtVersion(db, entityId, toVersionKey, spaceId),
					]),
				catch: (error) => new InternalError({message: String(error)}),
			}).pipe(Effect.withSpan("getEntitySnapshots"))

			// Compute diff
			const diff = diffEntitySnapshots(entityId, fromSnapshot, toSnapshot)

			return diff
		}).pipe(
			Effect.withSpan("GET /versioned/entities/:id/diff"),
			Effect.annotateLogs({entityId, fromEditId, toEditId}),
		)

		const result = await runtime.runPromise(Effect.either(program))

		return Either.match(result, {
			onLeft: (error: VersionedError) => {
				switch (error._tag) {
					case "ValidationError":
						return c.json({error: "Invalid parameter", message: error.message}, 400)
					case "NotFoundError":
						return c.json({error: "Not found", message: error.message}, 404)
					case "InternalError":
						return c.json({error: "Internal error", message: error.message}, 500)
				}
			},
			onRight: (diff: EntityDiff) => c.json(diff),
		})
	})

	return router
}
