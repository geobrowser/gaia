import {drizzle} from "drizzle-orm/node-postgres"
import {Context, Data, Effect, Redacted} from "effect"
import {Pool} from "pg"

import {Environment} from "../environment"
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

let _pool: Pool | null = null

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

let _drizzle: ReturnType<typeof drizzle<DbSchema>> | null = null

export const createDb = (connectionString: string) => {
	if (!_pool) {
		_pool = new Pool({
			connectionString,
			max: 80,
		})
	}

	if (!_drizzle) {
		_drizzle = drizzle({
			casing: "snake_case",
			client: _pool,
			schema: schemaDefinition,
		})
	}

	return _drizzle
}

interface StorageShape {
	use: <T>(fn: (client: ReturnType<typeof createDb>) => T) => Effect.Effect<Awaited<T>, StorageError, never>
}

export class Storage extends Context.Tag("Storage")<Storage, StorageShape>() {}

export const make = Effect.gen(function* () {
	const environment = yield* Environment

	const db = createDb(Redacted.value(environment.databaseUrl))

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