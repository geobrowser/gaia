import {drizzle} from "drizzle-orm/node-postgres"
import {Context, Data, Effect, Redacted} from "effect"
import {Pool} from "pg"

import {Environment} from "../environment"
import {
	entities,
	entityForeignValues,
	ipfsCache,
	propertiesEntityRelations,
	relations,
	relationsEntityRelations,
	values,
} from "./schema"

export class StorageError extends Data.TaggedError("StorageError")<{
	cause?: unknown
	message?: string
}> {}

let _pool: Pool | null = null

export const createDb = (connectionString: string) => {
	if (!_pool) {
		_pool = new Pool({
			connectionString,
			max: 80,
		})
	}

	return drizzle({
		casing: "snake_case",
		client: _pool,
		schema: {
			ipfsCache,
			entities,
			properties: values,
			relations: relations,

			entityForeignProperties: entityForeignValues,
			propertiesEntityRelations,
			relationsEntityRelations,
		},
	})
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
