import {Effect, Layer} from "effect"
import {Batching, make as makeBatching} from "~/src/services/storage/dataloaders"
import {NodeSdkLive} from "~/src/services/telemetry"
import {
	DataType,
	type QueryEntitiesArgs,
	type QueryEntityArgs,
	type QueryPropertiesArgs,
	type QueryRelationsArgs,
	type QuerySearchArgs,
	type QuerySpacesArgs,
	type QueryTypesArgs,
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

export const entities = (args: QueryEntitiesArgs) => {
	return Effect.runPromise(
		EntityResolvers.getEntities(args).pipe(
			Effect.withSpan("getEntities"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const entity = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getEntity(args.id).pipe(
			Effect.withSpan("getEntity"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityName = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getEntityName(args.id).pipe(
			Effect.withSpan("getEntityName"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityDescription = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getEntityDescription(args.id).pipe(
			Effect.withSpan("getEntityDescription"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entityTypes = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getEntityTypes(args.id).pipe(
			Effect.withSpan("getEntityTypes"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const entitySpaces = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getSpaces(args.id).pipe(
			Effect.withSpan("getSpaces"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const values = (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return Effect.runPromise(
		EntityResolvers.getValues(args.id, args.spaceId).pipe(
			Effect.withSpan("getValues"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const entityRelations = (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return Effect.runPromise(
		EntityResolvers.getRelations(args.id, args.spaceId).pipe(
			Effect.withSpan("getRelations"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const entityBacklinks = (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return Effect.runPromise(
		EntityResolvers.getBacklinks(args.id, args.spaceId).pipe(
			Effect.withSpan("getBacklinks"),
			Effect.annotateSpans({entityId: args.id, spaceId: args.spaceId}),
			provideDeps,
		),
	)
}

export const relations = (args: QueryRelationsArgs) => {
	return Effect.runPromise(
		EntityResolvers.getAllRelations(args).pipe(
			Effect.withSpan("getAllRelations"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const relation = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getRelation(args.id).pipe(
			Effect.withSpan("getRelation"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const property = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		PropertyResolvers.getProperty(args.id).pipe(
			Effect.withSpan("getProperty"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const propertiesForType = (typeId: string, args: QueryTypesArgs) => {
	return Effect.runPromise(
		PropertyResolvers.getPropertiesForType(typeId, args).pipe(
			Effect.withSpan("getPropertiesForType"),
			Effect.annotateSpans({typeId, ...args}),
			provideDeps,
		),
	)
}

export const propertyRelationValueTypes = (args: QueryEntityArgs & {dataType: DataType}) => {
	// Only relations can have a relation value type
	if (args.dataType !== DataType.Relation) {
		return []
	}

	return Effect.runPromise(
		PropertyResolvers.getPropertyRelationValueTypes(args.id).pipe(
			Effect.withSpan("getPropertyRelationValueTypes"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const propertyRenderableType = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		PropertyResolvers.getPropertyRenderableType(args.id).pipe(
			Effect.withSpan("getPropertyRenderableType"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const types = (args: QueryTypesArgs) => {
	return Effect.runPromise(
		TypeResolvers.getTypes(args).pipe(Effect.withSpan("getTypes"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const blocks = (args: QueryEntityArgs) => {
	return Effect.runPromise(
		EntityResolvers.getBlocks(args.id).pipe(
			Effect.withSpan("getBlocks"),
			Effect.annotateSpans({entityId: args.id}),
			provideDeps,
		),
	)
}

export const search = (args: QuerySearchArgs) => {
	return Effect.runPromise(
		SearchResolvers.search(args).pipe(Effect.withSpan("search"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const properties = (args: QueryPropertiesArgs) => {
	return Effect.runPromise(
		PropertyResolvers.getProperties(args).pipe(
			Effect.withSpan("getProperties"),
			Effect.annotateSpans({...args}),
			provideDeps,
		),
	)
}

export const spaces = (args: QuerySpacesArgs) => {
	return Effect.runPromise(
		SpaceResolvers.getSpaces(args).pipe(Effect.withSpan("getSpaces"), Effect.annotateSpans({...args}), provideDeps),
	)
}

export const space = (id: string) => {
	return Effect.runPromise(
		SpaceResolvers.getSpace(id).pipe(Effect.withSpan("getSpace"), Effect.annotateSpans({spaceId: id}), provideDeps),
	)
}

export const spaceEntity = (id: string) => {
	return Effect.runPromise(
		SpaceResolvers.getSpaceEntity(id).pipe(
			Effect.withSpan("getSpaceEntity"),
			Effect.annotateSpans({spaceId: id}),
			provideDeps,
		),
	)
}

export const meta = () => {
	return Effect.runPromise(MetaResolvers.getMeta().pipe(Effect.withSpan("Query.meta"), provideDeps))
}
