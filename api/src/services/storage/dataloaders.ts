import {SystemIds} from "@graphprotocol/grc-20"
import DataLoader from "dataloader"
import {Context, Data, Effect} from "effect"
import type {Entity} from "~/src/generated/graphql"
import {Storage} from "./storage"

export class BatchingError extends Data.TaggedError("BatchingError")<{
	cause?: unknown
	message?: string
}> {}

// Request-scoped batching service
interface BatchingShape {
	loadEntity: (id: string) => Effect.Effect<
		{
			createdAt: string
			createdAtBlock: string
			id: string
			updatedAt: string
			updatedAtBlock: string
		} | null,
		BatchingError,
		never
	>
	loadEntityName: (id: string) => Effect.Effect<string | null, BatchingError, never>
	loadEntityDescription: (id: string) => Effect.Effect<string | null, BatchingError, never>
	loadEntityValues: (
		entityId: string,
		spaceId?: string | null,
	) => Effect.Effect<
		{
			id: string
			propertyId: string
			entityId: string
			spaceId: string
			value: string
			language: string | null
			unit: string | null
		}[],
		BatchingError,
		never
	>
	loadEntityRelations: (
		entityId: string,
		spaceId?: string | null,
	) => Effect.Effect<
		{
			id: string
			entityId: string
			spaceId: string
			typeId: string
			fromEntityId: string
			fromSpaceId: string | null
			fromVersionId: string | null
			toEntityId: string
			toSpaceId: string | null
			toVersionId: string | null
			position: string | null
			verified: boolean | null
		}[],
		BatchingError,
		never
	>
	loadEntityBacklinks: (entityId: string, spaceId?: string | null) => Effect.Effect<Entity[], BatchingError, never>
	loadProperty: (propertyId: string) => Effect.Effect<
		{
			id: string
			type: "Text" | "Number" | "Checkbox" | "Time" | "Point" | "Relation"
		} | null,
		BatchingError,
		never
	>
}

export class Batching extends Context.Tag("Batching")<Batching, BatchingShape>() {}

export const make = Effect.gen(function* () {
	const storage = yield* Storage

	const entitiesLoader = new DataLoader(async (ids: readonly string[]) => {
		const result = await Effect.runPromise(
			storage
				.use(async (client) => {
					const entities = await client.query.entities.findMany({
						where: (entities, {inArray}) => inArray(entities.id, ids),
					})

					const entityMap = new Map(entities.map((e) => [e.id, e]))
					return ids.map((id) => entityMap.get(id) ?? null)
				})
				.pipe(Effect.withSpan("entitiesLoader"), Effect.annotateSpans({ids})),
		)
		return result
	})

	const entityNamesLoader = new DataLoader(async (ids: readonly string[]) => {
		const result = await Effect.runPromise(
			storage.use(async (client) => {
				const values = await client.query.values.findMany({
					where: (values, {inArray, and, eq}) =>
						and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.NAME_PROPERTY)),
					columns: {
						entityId: true,
						value: true,
					},
				})

				const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
				return ids.map((id) => valueMap.get(id) || null)
			}),
		)
		return result
	})

	const entityDescriptionsLoader = new DataLoader(async (ids: readonly string[]) => {
		const result = await Effect.runPromise(
			storage
				.use(async (client) => {
					const values = await client.query.values.findMany({
						where: (values, {inArray, and, eq}) =>
							and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.DESCRIPTION_PROPERTY)),
						columns: {
							entityId: true,
							value: true,
						},
					})

					const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
					return ids.map((id) => valueMap.get(id) || null)
				})
				.pipe(Effect.withSpan("entityDescriptionsLoader"), Effect.annotateSpans({ids})),
		)
		return result
	})

	const entityValuesLoader = new DataLoader(
		async (keys: readonly {entityId: string; spaceId?: string | null}[]) => {
			const result = await Effect.runPromise(
				storage
					.use(async (client) => {
						const entityIds = keys.map((k) => k.entityId)
						const allValues = await client.query.values.findMany({
							where: (values, {inArray}) => inArray(values.entityId, entityIds),
						})

						const valuesByEntity = new Map<string, (typeof allValues)[number][]>()
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
					})
					.pipe(Effect.withSpan("entityValuesLoader"), Effect.annotateSpans({keys})),
			)
			return result
		},
		{
			cacheKeyFn: (key) => `${key.entityId}:${key.spaceId || "null"}`,
		},
	)

	const entityRelationsLoader = new DataLoader(
		async (keys: readonly {entityId: string; spaceId?: string | null}[]) => {
			const result = await Effect.runPromise(
				storage
					.use(async (client) => {
						const entityIds = keys.map((k) => k.entityId)
						const allRelations = await client.query.relations.findMany({
							where: (relations, {inArray}) => inArray(relations.fromEntityId, entityIds),
						})

						const relationsByEntity = new Map<string, (typeof allRelations)[number][]>()
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
					})
					.pipe(Effect.withSpan("entityRelationsLoader"), Effect.annotateSpans({keys})),
			)
			return result
		},
		{
			cacheKeyFn: (key) => `relations:${key.entityId}:${key.spaceId || "null"}`,
		},
	)

	const entityBacklinksLoader = new DataLoader(
		async (keys: readonly {entityId: string; spaceId?: string | null}[]) => {
			const result = await Effect.runPromise(
				storage
					.use(async (client) => {
						const entityIds = keys.map((k) => k.entityId)
						const allBacklinks = await client.query.relations.findMany({
							where: (relations, {inArray}) => inArray(relations.toEntityId, entityIds),
						})

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
					})
					.pipe(Effect.withSpan("entityBacklinksLoader"), Effect.annotateSpans({keys})),
			)
			return result
		},
		{
			cacheKeyFn: (key) => `backlinks:${key.entityId}:${key.spaceId || "null"}`,
		},
	)

	const propertiesLoader = new DataLoader(async (ids: readonly string[]) => {
		const result = await Effect.runPromise(
			storage
				.use(async (client) => {
					const properties = await client.query.properties.findMany({
						where: (properties, {inArray}) => inArray(properties.id, ids),
					})

					const propertyMap = new Map(properties.map((p) => [p.id, p]))
					return ids.map((id) => propertyMap.get(id) || null)
				})
				.pipe(Effect.withSpan("propertiesLoader"), Effect.annotateSpans({ids})),
		)
		return result
	})

	return Batching.of({
		loadEntity: (id: string) =>
			Effect.tryPromise({
				try: () => entitiesLoader.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity ${id}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("loadEntity")),

		loadEntityName: (id: string) =>
			Effect.tryPromise({
				try: () => entityNamesLoader.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity name ${id}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("loadEntityName")),

		loadEntityDescription: (id: string) =>
			Effect.tryPromise({
				try: () => entityDescriptionsLoader.load(id),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity description ${id}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId: id}), Effect.withSpan("loadEntityDescription")),

		loadEntityValues: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityValuesLoader.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity values ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId}), Effect.withSpan("loadEntityValues")),

		loadEntityRelations: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityRelationsLoader.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity relations ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId}), Effect.withSpan("loadEntityRelations")),

		loadEntityBacklinks: (entityId: string, spaceId?: string | null) =>
			Effect.tryPromise({
				try: () => entityBacklinksLoader.load({entityId, spaceId}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity backlinks ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId}), Effect.withSpan("loadEntityBacklinks")),

		loadProperty: (propertyId: string) =>
			Effect.tryPromise({
				try: () => propertiesLoader.load(propertyId),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load property ${propertyId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({propertyId}), Effect.withSpan("loadProperty")),
	})
})
