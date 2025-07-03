import {drizzle} from "drizzle-orm/node-postgres"
import {Context, Data, Effect, Redacted} from "effect"
import {Pool} from "pg"

import {EnvironmentLive} from "../environment"
import {
	editors,
	editorsRelations,
	entities,
	entityForeignValues,
	ipfsCache,
	members,
	membersRelations,
	meta,
	properties,
	propertiesEntityRelations,
	propertiesRelations,
	relations,
	relationsEntityRelations,
	spaces,
	spacesRelations,
	values,
} from "./schema"

export class StorageError extends Data.TaggedError("StorageError")<{
	cause?: unknown
	message?: string
}> {}

const _pool = new Pool({
	connectionString: Redacted.value(EnvironmentLive.databaseUrl),
	max: 18,
	// min: 2,
	// idleTimeoutMillis: 30000,
	// connectionTimeoutMillis: 15000, // Slightly increased for batched queries
	// allowExitOnIdle: true, // Allow process to exit when pool is idle
})

// Add basic error handling for the pool
_pool.on("error", (err) => {
	console.error("PostgreSQL pool error:", err)
})

const schemaDefinition = {
	ipfsCache,
	entities,
	properties: properties,
	values: values,
	relations: relations,
	spaces,
	members,
	editors,
	meta,

	entityForeignProperties: entityForeignValues,
	propertiesEntityRelations,
	relationsEntityRelations,
	propertiesRelations,
	membersRelations,
	editorsRelations,
	spacesRelations,
} as const

type DbSchema = typeof schemaDefinition

export const db = drizzle<DbSchema>({
	casing: "snake_case",
	client: _pool,
	schema: schemaDefinition,
})

interface StorageShape {
	use: <T>(fn: (client: typeof db) => T) => Effect.Effect<Awaited<T>, StorageError, never>
	getPoolStats: () => Effect.Effect<
		{
			totalConnections: number
			idleConnections: number
			waitingCount: number
			maxConnections: number
		},
		never,
		never
	>
}

export class Storage extends Context.Tag("Storage")<Storage, StorageShape>() {}

export const make = Effect.gen(function* () {
	return Storage.of({
		use: (fn) => {
			return Effect.gen(function* () {
				const result = yield* Effect.try({
					try: () => fn(db),
					catch: (error) => {
						const errorMessage = String(error)

						// Provide more specific error messages for common pool issues
						if (errorMessage.includes("too many clients")) {
							return new StorageError({
								message: `Database connection pool exhausted. Consider increasing max pool size or optimizing query patterns.`,
								cause: error,
							})
						}

						if (errorMessage.includes("pool is closed")) {
							return new StorageError({
								message: `Database connection pool is closed.`,
								cause: error,
							})
						}

						return new StorageError({
							message: `Database operation failed: ${errorMessage}`,
							cause: error,
						})
					},
				})

				if (result instanceof Promise) {
					return yield* Effect.tryPromise({
						try: () => result,
						catch: (error) => {
							const errorMessage = String(error)

							if (errorMessage.includes("too many clients")) {
								return new StorageError({
									cause: error,
									message: `Database connection pool exhausted. Consider increasing max pool size or optimizing query patterns.`,
								})
							}

							if (errorMessage.includes("pool is closed")) {
								return new StorageError({
									cause: error,
									message: `Database connection pool is closed.`,
								})
							}

							return new StorageError({
								cause: error,
								message: `Async database operation failed: ${errorMessage}`,
							})
						},
					})
				}

				return result
			})
		},

		getPoolStats: () => {
			return Effect.sync(() => ({
				totalConnections: _pool.totalCount,
				idleConnections: _pool.idleCount,
				waitingCount: _pool.waitingCount,
				maxConnections: _pool.options.max || 10,
			}))
		},
	})
})
