import {drizzle} from "drizzle-orm/node-postgres"
import {Context, Data, Effect, Redacted} from "effect"
import {Pool} from "pg"

import {EnvironmentLive} from "../environment"
import {
	entities,
	entityForeignValues,
	ipfsCache,
	properties,
	propertiesEntityRelations,
	propertiesRelations,
	relations,
	relationsEntityRelations,
	spaces,
	values,
} from "./schema"

export class StorageError extends Data.TaggedError("StorageError")<{
	cause?: unknown
	message?: string
}> {}

const _pool = new Pool({
	connectionString: Redacted.value(EnvironmentLive.databaseUrl),
	max: 97,
})

const schemaDefinition = {
	ipfsCache,
	entities,
	properties: properties,
	values: values,
	relations: relations,
	spaces,

	entityForeignProperties: entityForeignValues,
	propertiesEntityRelations,
	relationsEntityRelations,
	propertiesRelations,
} as const

type DbSchema = typeof schemaDefinition

const db = drizzle<DbSchema>({
	casing: "snake_case",
	client: _pool,
	schema: schemaDefinition,
})

interface StorageShape {
	use: <T>(fn: (client: typeof db) => T) => Effect.Effect<Awaited<T>, StorageError, never>
}

export class Storage extends Context.Tag("Storage")<Storage, StorageShape>() {}

export const make = Effect.gen(function* () {
	return Storage.of({
		use: (fn) => {
			return Effect.gen(function* () {
				const result = yield* Effect.try({
					try: () => fn(db),
					catch: (error) =>
						new StorageError({message: `Synchronous error in Db.use ${String(error)}`, cause: error}),
				})

				if (result instanceof Promise) {
					return yield* Effect.tryPromise({
						try: () => result,
						catch: (error) =>
							new StorageError({
								cause: error,
								message: `Asynchronous error in Db.use ${String(error)}`,
							}),
					})
				}

				return result
			})
		},
	})
})
