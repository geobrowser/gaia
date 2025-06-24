import {SystemIds} from "@graphprotocol/grc-20"
import {Context, Data, Effect} from "effect"
import {Storage} from "./storage"

export class BatchingError extends Data.TaggedError("BatchingError")<{
	cause?: unknown
	message?: string
}> {}

// Simple batching utility that collects requests and executes them together
class SimpleBatcher<K, V> {
	private pending = new Map<string, {keys: K[]; resolve: (values: V[]) => void; reject: (error: any) => void}>()
	private batchTimeout: NodeJS.Timeout | null = null

	constructor(
		private batchFn: (keys: K[]) => Promise<V[]>,
		private keyFn: (key: K) => string,
		private maxBatchSize = 100,
		private batchDelayMs = 5,
	) {}

	load(key: K): Promise<V> {
		const keyStr = this.keyFn(key)

		return new Promise<V>((resolve, reject) => {
			// Add to pending batch
			if (!this.pending.has(keyStr)) {
				this.pending.set(keyStr, {keys: [], resolve: () => {}, reject: () => {}})
			}

			const batch = this.pending.get(keyStr)!
			batch.keys.push(key)

			const originalResolve = batch.resolve
			const originalReject = batch.reject

			batch.resolve = (values: V[]) => {
				const index = batch.keys.indexOf(key)
				if (index >= 0 && index < values.length) {
					const value = values[index]
					if (value !== undefined) {
						resolve(value)
					} else {
						reject(new Error(`Undefined value for key: ${keyStr}`))
					}
				} else {
					reject(new Error(`Value not found for key: ${keyStr}`))
				}
				originalResolve(values)
			}

			batch.reject = (error: any) => {
				reject(error)
				originalReject(error)
			}

			// Schedule batch execution
			this.scheduleBatch()
		})
	}

	private scheduleBatch() {
		if (this.batchTimeout) return

		this.batchTimeout = setTimeout(() => {
			this.executeBatch()
		}, this.batchDelayMs)
	}

	private async executeBatch() {
		this.batchTimeout = null
		const batches = Array.from(this.pending.entries())
		this.pending.clear()

		// Group all keys together
		const allKeys: K[] = []
		const resolvers: Array<{resolve: (values: any[]) => void; reject: (error: any) => void}> = []

		for (const [_, batch] of batches) {
			allKeys.push(...batch.keys)
			resolvers.push({resolve: batch.resolve, reject: batch.reject})
		}

		if (allKeys.length === 0) return

		try {
			const results = await this.batchFn(allKeys)
			resolvers.forEach(({resolve}) => resolve(results))
		} catch (error) {
			resolvers.forEach(({reject}) => reject(error))
		}
	}
}

// Request-scoped batching service
interface BatchingShape {
	loadEntity: (id: string) => Effect.Effect<any, BatchingError, never>
	loadEntityName: (id: string) => Effect.Effect<string | null, BatchingError, never>
	loadEntityDescription: (id: string) => Effect.Effect<string | null, BatchingError, never>
	loadEntityValues: (entityId: string, spaceId?: string | null) => Effect.Effect<any[], BatchingError, never>
	loadEntityRelations: (entityId: string, spaceId?: string | null) => Effect.Effect<any[], BatchingError, never>
	loadEntityBacklinks: (entityId: string, spaceId?: string | null) => Effect.Effect<any[], BatchingError, never>
	loadProperty: (propertyId: string) => Effect.Effect<any, BatchingError, never>
}

export class Batching extends Context.Tag("Batching")<Batching, BatchingShape>() {}

export const make = Effect.gen(function* () {
	const storage = yield* Storage

	// Create batchers
	const entitiesBatcher = new SimpleBatcher(
		async (ids: string[]) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const entities = await client.query.entities.findMany({
						where: (entities, {inArray}) => inArray(entities.id, ids),
					})

					// Create lookup map and return results in same order as input
					const entityMap = new Map(entities.map((e) => [e.id, e]))
					return ids.map((id) => entityMap.get(id) || null)
				}),
			)
			return result
		},
		(id: string) => id,
		50,
	)

	const entityNamesBatcher = new SimpleBatcher(
		async (ids: string[]) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const values = await client.query.values.findMany({
						where: (values, {inArray, and, eq}) =>
							and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.NAME_PROPERTY)),
					})

					// Create lookup map and return results in same order as input
					const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
					return ids.map((id) => valueMap.get(id) || null)
				}),
			)
			return result
		},
		(id: string) => id,
		50,
	)

	const entityDescriptionsBatcher = new SimpleBatcher(
		async (ids: string[]) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const values = await client.query.values.findMany({
						where: (values, {inArray, and, eq}) =>
							and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.DESCRIPTION_PROPERTY)),
					})

					// Create lookup map and return results in same order as input
					const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
					return ids.map((id) => valueMap.get(id) || null)
				}),
			)
			return result
		},
		(id: string) => id,
		50,
	)

	const entityValuesBatcher = new SimpleBatcher(
		async (keys: Array<{entityId: string; spaceId?: string | null}>) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const entityIds = keys.map((k) => k.entityId)
					const allValues = await client.query.values.findMany({
						where: (values, {inArray}) => inArray(values.entityId, entityIds),
					})

					// Group by entityId and filter by spaceId if needed
					const valuesByEntity = new Map<string, any[]>()
					for (const value of allValues) {
						if (!valuesByEntity.has(value.entityId)) {
							valuesByEntity.set(value.entityId, [])
						}
						valuesByEntity.get(value.entityId)!.push(value)
					}

					return keys.map((key) => {
						const entityValues = valuesByEntity.get(key.entityId) || []
						if (key.spaceId) {
							return entityValues.filter((v) => v.spaceId === key.spaceId)
						}
						return entityValues
					})
				}),
			)
			return result
		},
		(key: {entityId: string; spaceId?: string | null}) => `${key.entityId}:${key.spaceId || "null"}`,
		30,
	)

	const entityRelationsBatcher = new SimpleBatcher(
		async (keys: Array<{entityId: string; spaceId?: string | null}>) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const entityIds = keys.map((k) => k.entityId)
					const allRelations = await client.query.relations.findMany({
						where: (relations, {inArray}) => inArray(relations.fromEntityId, entityIds),
					})

					// Group by fromEntityId and filter by spaceId if needed
					const relationsByEntity = new Map<string, any[]>()
					for (const relation of allRelations) {
						if (!relationsByEntity.has(relation.fromEntityId)) {
							relationsByEntity.set(relation.fromEntityId, [])
						}
						relationsByEntity.get(relation.fromEntityId)!.push(relation)
					}

					return keys.map((key) => {
						const entityRelations = relationsByEntity.get(key.entityId) || []
						if (key.spaceId) {
							return entityRelations.filter((r) => r.spaceId === key.spaceId)
						}
						return entityRelations
					})
				}),
			)
			return result
		},
		(key: {entityId: string; spaceId?: string | null}) => `relations:${key.entityId}:${key.spaceId || "null"}`,
		30,
	)

	const entityBacklinksBatcher = new SimpleBatcher(
		async (keys: Array<{entityId: string; spaceId?: string | null}>) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const entityIds = keys.map((k) => k.entityId)
					const allBacklinks = await client.query.relations.findMany({
						where: (relations, {inArray}) => inArray(relations.toEntityId, entityIds),
					})

					// Group by toEntityId and filter by spaceId if needed
					const backlinksByEntity = new Map<string, any[]>()
					for (const backlink of allBacklinks) {
						if (!backlinksByEntity.has(backlink.toEntityId)) {
							backlinksByEntity.set(backlink.toEntityId, [])
						}
						backlinksByEntity.get(backlink.toEntityId)!.push(backlink)
					}

					return keys.map((key) => {
						const entityBacklinks = backlinksByEntity.get(key.entityId) || []
						if (key.spaceId) {
							return entityBacklinks.filter((r) => r.spaceId === key.spaceId)
						}
						return entityBacklinks
					})
				}),
			)
			return result
		},
		(key: {entityId: string; spaceId?: string | null}) => `backlinks:${key.entityId}:${key.spaceId || "null"}`,
		30,
	)

	const propertiesBatcher = new SimpleBatcher(
		async (ids: string[]) => {
			const result = await Effect.runPromise(
				storage.use(async (client) => {
					const properties = await client.query.properties.findMany({
						where: (properties, {inArray}) => inArray(properties.id, ids),
					})

					// Create lookup map and return results in same order as input
					const propertyMap = new Map(properties.map((p) => [p.id, p]))
					return ids.map((id) => propertyMap.get(id) || null)
				}),
			)
			return result
		},
		(id: string) => id,
		50,
	)

	return Batching.of({
		loadEntity: (id: string) =>
			Effect.tryPromise({
				try: () => entitiesBatcher.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity ${id}: ${String(error)}`,
					}),
			}),

		loadEntityName: (id: string) =>
			Effect.tryPromise({
				try: () => entityNamesBatcher.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity name ${id}: ${String(error)}`,
					}),
			}),

		loadEntityDescription: (id: string) =>
			Effect.tryPromise({
				try: () => entityDescriptionsBatcher.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity description ${id}: ${String(error)}`,
					}),
			}),

		loadEntityValues: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityValuesBatcher.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity values ${entityId}: ${String(error)}`,
					}),
			}),

		loadEntityRelations: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityRelationsBatcher.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity relations ${entityId}: ${String(error)}`,
					}),
			}),

		loadEntityBacklinks: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityBacklinksBatcher.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity backlinks ${entityId}: ${String(error)}`,
					}),
			}),

		loadProperty: (propertyId: string) =>
			Effect.tryPromise({
				try: () => propertiesBatcher.load(propertyId),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load property ${propertyId}: ${String(error)}`,
					}),
			}),
	})
})
