import {SystemIds} from "@graphprotocol/grc-20"
import {makeExecutableSchema} from "@graphql-tools/schema"
import {useResponseCache} from "@graphql-yoga/plugin-response-cache"
import {file} from "bun"
import DataLoader from "dataloader"
import {Effect, Layer} from "effect"
import {createYoga, useExecutionCancellation} from "graphql-yoga"
import type {
	DataType,
	EntityRelationsArgs,
	EntityValuesArgs,
	Resolvers as GeneratedResolvers,
	QuerySearchArgs,
	QuerySpaceArgs,
	QuerySpacesArgs,
	RelationFilter,
	ValueFilter,
} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"

import {db, make as makeStorage, Storage} from "../services/storage/storage"
import {NodeSdkLive} from "../services/telemetry"
import type {GraphQLContext} from "../types"
import * as MembershipResolvers from "./resolvers/membership"
import * as Resolvers from "./resolvers/root"

/**
 * Currently hand-rolling a compression polyfill until Bun implements
 * CompressionStream in the runtime.
 * https://github.com/oven-sh/bun/issues/1723
 */
import "./compression-polyfill"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))

const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

const schemaFile = await file("./schema.graphql").text()

const resolvers: GeneratedResolvers = {
	Query: {
		meta: async () => {
			return Resolvers.meta()
		},
		entities: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.entities(args)
		},
		entity: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.entity(args, context)
		},
		types: async (_, args, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.types(args)
		},
		search: (_, args: QuerySearchArgs, context: GraphQLContext) => {
			context.spaceId = args.spaceId
			return Resolvers.search(args)
		},
		properties: (_, args) => {
			return Resolvers.properties(args)
		},
		property: (_, args, context: GraphQLContext) => {
			return Resolvers.property({id: args.id}, context)
		},
		spaces: (_, args: QuerySpacesArgs) => {
			return Resolvers.spaces(args)
		},
		space: (_, args: QuerySpaceArgs) => {
			return Resolvers.space(args.id)
		},
		relation: (_, args) => {
			return Resolvers.relation({id: args.id})
		},
		relations: (_, args) => {
			return Resolvers.relations(args)
		},
	},
	Entity: {
		name: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.entityName({id: parent.id}, context)
		},
		description: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entityDescription({id: parent.id}, context)
		},
		blocks: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.blocks({id: parent.id}, context)
		},
		types: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.entityTypes({id: parent.id}, context)
		},
		spaces: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.entitySpaces({id: parent.id}, context)
		},
		values: (parent: {id: string}, args: EntityValuesArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.values({id: parent.id, spaceId}, context)
		},
		relations: (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityRelations(
				{
					id: parent.id,
					spaceId,
				},
				context,
			)
		},
		backlinks: (parent: {id: string}, args: EntityRelationsArgs, context: GraphQLContext) => {
			const spaceId = args.spaceId ?? context.spaceId
			return Resolvers.entityBacklinks(
				{
					id: parent.id,
					spaceId: spaceId,
				},
				context,
			)
		},
	},
	Type: {
		name: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.entityName({id: parent.id}, context)
		},
		description: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entityDescription({id: parent.id}, context)
		},
		entity: (parent: {id: string}, _, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.id}, context)
		},
		properties: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.propertiesForType(
				parent.id,
				{
					spaceId: context.spaceId,
				},
				context,
			)
		},
	},
	Value: {
		entity: (parent: {entityId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.entityId}, context)
		},
		property: (parent: {propertyId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.property(
				{
					id: parent.propertyId,
				},
				context,
			)
		},
	},
	Property: {
		entity: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.id}, context)
		},
		relationValueTypes: (parent: {id: string; dataType: DataType}, _: unknown, context: GraphQLContext) => {
			return Resolvers.propertyRelationValueTypes(
				{
					id: parent.id,
					dataType: parent.dataType,
				},
				context,
			)
		},
		renderableType: (parent: {id: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.propertyRenderableType({id: parent.id}, context)
		},
	},
	Relation: {
		from: (parent: {fromId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.fromId}, context)
		},
		to: (parent: {toId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.toId}, context)
		},
		type: (parent: {typeId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.property({id: parent.typeId}, context)
		},
		relationEntity: async (parent: {entityId: string}, _: unknown, context: GraphQLContext) => {
			return Resolvers.entity({id: parent.entityId}, context)
		},
	},
	Space: {
		entity: (parent: {id: string}) => {
			return Resolvers.spaceEntity(parent.id)
		},
		editors: (parent: {id: string}) => {
			return Effect.runPromise(MembershipResolvers.getEditors({spaceId: parent.id}).pipe(provideDeps))
		},
		members: (parent: {id: string}) => {
			return Effect.runPromise(MembershipResolvers.getMembers({spaceId: parent.id}).pipe(provideDeps))
		},
	},
}

const schema = makeExecutableSchema({
	typeDefs: schemaFile,
	resolvers,
})

export const graphqlServer = createYoga({
	schema,
	batching: true,
	plugins: [
		useExecutionCancellation(),
		useResponseCache({
			session: () => null,
			ttl: 10_000, // 10 seconds
		}),
	],
	context: (): GraphQLContext => {
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
				maxBatchSize: 100,
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
				maxBatchSize: 100,
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
				maxBatchSize: 100,
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

					return keys.map(
						(key) => {
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
						},
						{
							maxBatchSize: 100,
						},
					)
				}

				return Effect.runPromise(
					Effect.promise(batch).pipe(
						Effect.withSpan("entityValuesLoader"),
						Effect.annotateSpans({keys, batchLength: keys.length}),
					),
				)
			},
			{
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

					return keys.map(
						(key) => {
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
									entityRelations = entityRelations.filter(
										(r) => r.fromEntityId === key.filter?.fromEntityId,
									)
								}
								if (key.filter.toEntityId && key.filter.toEntityId !== "") {
									entityRelations = entityRelations.filter(
										(r) => r.toEntityId === key.filter?.toEntityId,
									)
								}
								if (key.filter.relationEntityId && key.filter.relationEntityId !== "") {
									entityRelations = entityRelations.filter(
										(r) => r.entityId === key.filter?.relationEntityId,
									)
								}
							}

							return entityRelations
						},
						{
							maxBatchSize: 100,
						},
					)
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
				maxBatchSize: 100,
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
				maxBatchSize: 100,
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
								entityBacklinks = entityBacklinks.filter(
									(r) => r.fromEntityId === key.filter?.fromEntityId,
								)
							}
							if (key.filter.toEntityId && key.filter.toEntityId !== "") {
								entityBacklinks = entityBacklinks.filter((r) => r.toEntityId === key.filter?.toEntityId)
							}
							if (key.filter.relationEntityId && key.filter.relationEntityId !== "") {
								entityBacklinks = entityBacklinks.filter(
									(r) => r.entityId === key.filter?.relationEntityId,
								)
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
				maxBatchSize: 100,
				cacheKeyFn: (key) =>
					`backlinks:${key.entityId}:${key.spaceId || "null"}:${JSON.stringify(key.filter) || "null"}`,
			},
		)

		return {
			entitiesLoader,
			entityNamesLoader,
			entityDescriptionsLoader,
			entityValuesLoader,
			entityRelationsLoader,
			propertiesLoader,
			entityBacklinksLoader,
		}
	},
	graphqlEndpoint: "/graphql",
	fetchAPI: {Response, Request},
})
