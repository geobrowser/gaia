import {Effect, Layer} from "effect"
import {Batching, make as makeBatching} from "~/src/services/storage/dataloaders"
import {NodeSdkLive} from "~/src/services/telemetry"
import type {
	QueryEntitiesArgs,
	QueryEntityArgs,
	QueryPropertiesArgs,
	QueryRelationsArgs,
	QuerySearchArgs,
	QuerySpacesArgs,
	QueryTypesArgs,
} from "../../generated/graphql"
import {Environment, make as makeEnvironment} from "../../services/environment"
import {make as makeStorage, Storage} from "../../services/storage/storage"
import * as EntityResolvers from "./entities"
import * as MetaResolvers from "./meta"
import * as PropertyResolvers from "./properties"
import * as SearchResolvers from "./search"
import * as SpaceResolvers from "./spaces"
import * as TypeResolvers from "./types"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const BatchingLayer = Layer.effect(Batching, makeBatching).pipe(Layer.provide(StorageLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer, BatchingLayer, NodeSdkLive)
const provideDeps = Effect.provide(layers)

export const entities = async (args: QueryEntitiesArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntities(args).pipe(
			Effect.withSpan("getEntities"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const entity = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntity(args.id).pipe(
			Effect.withSpan("getEntity"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityName = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntityName(args.id).pipe(
			Effect.withSpan("getEntityName"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityDescription = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntityDescription(args.id).pipe(
			Effect.withSpan("getEntityDescription"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityTypes = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntityTypes(args.id).pipe(
			Effect.withSpan("getEntityTypes"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entitySpaces = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getSpaces(args.id).pipe(
			Effect.withSpan("getSpaces"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const values = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(
		EntityResolvers.getValues(args.id, args.spaceId).pipe(
			Effect.withSpan("getValues"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const entityRelations = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(
		EntityResolvers.getRelations(args.id, args.spaceId).pipe(
			Effect.withSpan("getRelations"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const entityBacklinks = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(
		EntityResolvers.getBacklinks(args.id, args.spaceId).pipe(
			Effect.withSpan("getBacklinks"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const relations = async (args: QueryRelationsArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getAllRelations(args).pipe(
			Effect.withSpan("getAllRelations"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const relation = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getRelation(args.id).pipe(
			Effect.withSpan("getRelation"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const property = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		PropertyResolvers.getProperty(args.id).pipe(
			Effect.withSpan("getProperty"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const propertiesForType = async (typeId: string, args: QueryTypesArgs) => {
	return await Effect.runPromise(
		PropertyResolvers.getPropertiesForType(typeId, args).pipe(
			Effect.withSpan("getPropertiesForType"),
			Effect.annotateSpans({typeId, ...args}),
			provideDeps,
		),
	)
}

export const propertyRelationValueTypes = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		PropertyResolvers.getPropertyRelationValueTypes(args.id).pipe(
			Effect.withSpan("getPropertyRelationValueTypes"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const propertyRenderableType = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		PropertyResolvers.getPropertyRenderableType(args.id).pipe(
			Effect.withSpan("getPropertyRenderableType"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const types = async (args: QueryTypesArgs) => {
	return await Effect.runPromise(
		TypeResolvers.getTypes(args).pipe(Effect.withSpan("getTypes"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const blocks = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getBlocks(args.id).pipe(
			Effect.withSpan("getBlocks"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const search = async (args: QuerySearchArgs) => {
	return await Effect.runPromise(
		SearchResolvers.search(args).pipe(Effect.withSpan("search"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const properties = async (args: QueryPropertiesArgs) => {
	return await Effect.runPromise(
		PropertyResolvers.getProperties(args).pipe(
			Effect.withSpan("getProperties"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const spaces = async (args: QuerySpacesArgs) => {
	return await Effect.runPromise(
		SpaceResolvers.getSpaces(args).pipe(Effect.withSpan("getSpaces"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const space = async (id: string) => {
	return await Effect.runPromise(
		SpaceResolvers.getSpace(id).pipe(Effect.withSpan("getSpace"), Effect.annotateSpans({spaceId: id}), provideDeps),
	)
}

export const spaceEntity = async (id: string) => {
	return await Effect.runPromise(
		SpaceResolvers.getSpaceEntity(id).pipe(
			Effect.withSpan("getSpaceEntity"),
			Effect.annotateSpans({spaceId: id}),
			provideDeps,
		),
	)
}

export const meta = async () => {
	return await Effect.runPromise(MetaResolvers.getMeta().pipe(Effect.withSpan("Query.meta"), provideDeps))
}
