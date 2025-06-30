import {SystemIds} from "@graphprotocol/grc-20"
import DataLoader from "dataloader"
import {Context, Data, Effect} from "effect"
import {LRUMap} from "lru_map"
import type {RelationFilter, ValueFilter} from "~/src/generated/graphql"
import {NodeSdkLive} from "../telemetry"
import {db} from "./storage"

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
		filter?: ValueFilter | null,
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
		filter?: RelationFilter | null,
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
	loadEntityBacklinks: (
		entityId: string,
		spaceId?: string | null,
		filter?: RelationFilter | null,
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

		loadEntityValues: (entityId: string, spaceId?: string | null, filter?: ValueFilter | null) =>
			Effect.tryPromise({
				try: () => entityValuesLoader.load({entityId, spaceId, filter}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity values ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId, filter}), Effect.withSpan("loadEntityValues")),

		loadEntityRelations: (entityId: string, spaceId?: string | null, filter?: RelationFilter | null) =>
			Effect.tryPromise({
				try: () => entityRelationsLoader.load({entityId, spaceId, filter}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity relations ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId, filter}), Effect.withSpan("loadEntityRelations")),

		loadEntityBacklinks: (entityId: string, spaceId?: string | null, filter?: RelationFilter | null) =>
			Effect.tryPromise({
				try: () => entityBacklinksLoader.load({entityId, spaceId, filter}),
				catch: (error) =>
					new BatchingError({
						cause: error,
						message: `Failed to batch load entity backlinks ${entityId}: ${String(error)}`,
					}),
			}).pipe(Effect.annotateSpans({entityId, spaceId, filter}), Effect.withSpan("loadEntityBacklinks")),

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

const entitiesLoader = new DataLoader(
	(ids: readonly string[]) => {
		const batch = async () => {
			const entities = await db.query.entities.findMany({
				where: (entities, {inArray}) => inArray(entities.id, ids),
			})

			const entityMap = new Map(entities.map((e) => [e.id, e]))
			return ids.map((id) => entityMap.get(id) ?? null)
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entitiesLoader"),
				Effect.annotateSpans({ids, batchLength: ids.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(10000),
	},
)

const entityNamesLoader = new DataLoader(
	(ids: readonly string[]) => {
		const batch = async () => {
			const values = await db.query.values.findMany({
				where: (values, {inArray, and, eq}) =>
					and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.NAME_PROPERTY)),
				columns: {
					entityId: true,
					value: true,
				},
			})

			const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
			return ids.map((id) => valueMap.get(id) || null)
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entityNamesLoader"),
				Effect.annotateSpans({ids, batchLength: ids.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(10000),
	},
)

const entityDescriptionsLoader = new DataLoader(
	(ids: readonly string[]) => {
		const batch = async () => {
			const values = await db.query.values.findMany({
				where: (values, {inArray, and, eq}) =>
					and(inArray(values.entityId, ids), eq(values.propertyId, SystemIds.DESCRIPTION_PROPERTY)),
				columns: {
					entityId: true,
					value: true,
				},
			})

			const valueMap = new Map(values.map((v) => [v.entityId, v.value]))
			return ids.map((id) => valueMap.get(id) || null)
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entityDescriptionsLoader"),
				Effect.annotateSpans({ids, batchLength: ids.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(10000),
	},
)

const entityValuesLoader = new DataLoader(
	(
		keys: readonly {
			entityId: string
			spaceId?: string | null
			filter?: ValueFilter | null
		}[],
	) => {
		const batch = async () => {
			const entityIds = keys.map((k) => k.entityId)
			const allValues = await db.query.values.findMany({
				where: (values, {inArray}) => inArray(values.entityId, entityIds),
			})

			const valuesByEntity = new Map<string, (typeof allValues)[number][]>()
			for (const value of allValues) {
				if (!valuesByEntity.has(value.entityId)) {
					valuesByEntity.set(value.entityId, [])
				}
				valuesByEntity.get(value.entityId)?.push(value)
			}

			return keys.map((key) => {
				let entityValues = valuesByEntity.get(key.entityId) || []

				// Apply spaceId filter
				if (key.spaceId) {
					entityValues = entityValues.filter((v) => v.spaceId === key.spaceId)
				}

				// Apply value filter
				if (key.filter?.property) {
					entityValues = entityValues.filter((v) => v.propertyId === key.filter?.property)
				}

				return entityValues
			})
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entityValuesLoader"),
				Effect.annotateSpans({keys, batchLength: keys.length}),
			),
		)
	},
	{
		cacheMap: new LRUMap(1000),
		cacheKeyFn: (key) => `${key.entityId}:${key.spaceId || "null"}:${JSON.stringify(key.filter) || "null"}`,
	},
)

const entityRelationsLoader = new DataLoader(
	(
		keys: readonly {
			entityId: string
			spaceId?: string | null
			filter?: RelationFilter | null
		}[],
	) => {
		const batch = async () => {
			const entityIds = keys.map((k) => k.entityId)
			const allRelations = await db.query.relations.findMany({
				where: (relations, {inArray}) => inArray(relations.fromEntityId, entityIds),
			})

			const relationsByEntity = new Map<string, (typeof allRelations)[number][]>()
			for (const relation of allRelations) {
				if (!relationsByEntity.has(relation.fromEntityId)) {
					relationsByEntity.set(relation.fromEntityId, [])
				}
				relationsByEntity.get(relation.fromEntityId)?.push(relation)
			}

			return keys.map((key) => {
				let entityRelations = relationsByEntity.get(key.entityId) || []

				// Apply spaceId filter
				if (key.spaceId) {
					entityRelations = entityRelations.filter((r) => r.spaceId === key.spaceId)
				}

				// Apply relation filters
				if (key.filter) {
					if (key.filter.typeId && key.filter.typeId !== "") {
						entityRelations = entityRelations.filter((r) => r.typeId === key.filter?.typeId)
					}
					if (key.filter.fromEntityId && key.filter.fromEntityId !== "") {
						entityRelations = entityRelations.filter((r) => r.fromEntityId === key.filter?.fromEntityId)
					}
					if (key.filter.toEntityId && key.filter.toEntityId !== "") {
						entityRelations = entityRelations.filter((r) => r.toEntityId === key.filter?.toEntityId)
					}
					if (key.filter.relationEntityId && key.filter.relationEntityId !== "") {
						entityRelations = entityRelations.filter((r) => r.entityId === key.filter?.relationEntityId)
					}
				}

				return entityRelations
			})
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entityRelationsLoader"),
				Effect.annotateSpans({keys, batchLength: keys.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(1000),
		cacheKeyFn: (key) =>
			`relations:${key.entityId}:${key.spaceId || "null"}:${JSON.stringify(key.filter) || "null"}`,
	},
)

const propertiesLoader = new DataLoader(
	(ids: readonly string[]) => {
		const batch = async () => {
			const properties = await db.query.properties.findMany({
				where: (properties, {inArray}) => inArray(properties.id, ids),
			})

			const propertyMap = new Map(properties.map((p) => [p.id, p]))
			return ids.map((id) => propertyMap.get(id) || null)
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("propertiesLoader"),
				Effect.annotateSpans({ids, batchLength: ids.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(1000),
	},
)

const entityBacklinksLoader = new DataLoader(
	(
		keys: readonly {
			entityId: string
			spaceId?: string | null
			filter?: RelationFilter | null
		}[],
	) => {
		const batch = async () => {
			const entityIds = keys.map((k) => k.entityId)
			const allBacklinks = await db.query.relations.findMany({
				where: (relations, {inArray}) => inArray(relations.toEntityId, entityIds),
			})

			const backlinksByEntity = new Map<string, (typeof allBacklinks)[number][]>()
			for (const backlink of allBacklinks) {
				if (!backlinksByEntity.has(backlink.toEntityId)) {
					backlinksByEntity.set(backlink.toEntityId, [])
				}
				backlinksByEntity.get(backlink.toEntityId)?.push(backlink)
			}

			return keys.map((key) => {
				let entityBacklinks = backlinksByEntity.get(key.entityId) || []

				// Apply spaceId filter
				if (key.spaceId) {
					entityBacklinks = entityBacklinks.filter((r) => r.spaceId === key.spaceId)
				}

				// Apply relation filters
				if (key.filter) {
					if (key.filter.typeId && key.filter.typeId !== "") {
						entityBacklinks = entityBacklinks.filter((r) => r.typeId === key.filter?.typeId)
					}
					if (key.filter.fromEntityId && key.filter.fromEntityId !== "") {
						entityBacklinks = entityBacklinks.filter((r) => r.fromEntityId === key.filter?.fromEntityId)
					}
					if (key.filter.toEntityId && key.filter.toEntityId !== "") {
						entityBacklinks = entityBacklinks.filter((r) => r.toEntityId === key.filter?.toEntityId)
					}
					if (key.filter.relationEntityId && key.filter.relationEntityId !== "") {
						entityBacklinks = entityBacklinks.filter((r) => r.entityId === key.filter?.relationEntityId)
					}
				}

				return entityBacklinks
			})
		}

		return Effect.runPromise(
			Effect.promise(batch).pipe(
				Effect.withSpan("entityBacklinksLoader"),
				Effect.annotateSpans({keys, batchLength: keys.length}),
				Effect.provide(NodeSdkLive),
			),
		)
	},
	{
		cacheMap: new LRUMap(1000),
		cacheKeyFn: (key) =>
			`backlinks:${key.entityId}:${key.spaceId || "null"}:${JSON.stringify(key.filter) || "null"}`,
	},
)
