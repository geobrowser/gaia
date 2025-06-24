import {Effect, Layer} from "effect"
import {Batching, make as makeBatching} from "~/src/services/storage/dataloaders"
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
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer, BatchingLayer)
const provideDeps = Effect.provide(layers)

export const entities = async (args: QueryEntitiesArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntities(args).pipe(provideDeps))
}

export const entity = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntity(args.id).pipe(provideDeps))
}

export const entityName = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntityName(args.id).pipe(provideDeps))
}

export const entityDescription = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntityDescription(args.id).pipe(provideDeps))
}

export const entityTypes = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntityTypes(args.id).pipe(provideDeps))
}

export const entitySpaces = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getSpaces(args.id).pipe(provideDeps))
}

export const values = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(EntityResolvers.getValues(args.id, args.spaceId).pipe(provideDeps))
}

export const entityRelations = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(EntityResolvers.getRelations(args.id, args.spaceId).pipe(provideDeps))
}

export const entityBacklinks = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(EntityResolvers.getBacklinks(args.id, args.spaceId).pipe(provideDeps))
}

export const relations = async (args: QueryRelationsArgs) => {
	return await Effect.runPromise(EntityResolvers.getAllRelations(args).pipe(provideDeps))
}

export const relation = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getRelation(args.id).pipe(provideDeps))
}

export const property = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(PropertyResolvers.getProperty(args.id).pipe(provideDeps))
}

export const propertiesForType = async (typeId: string, args: QueryTypesArgs) => {
	return await Effect.runPromise(PropertyResolvers.getPropertiesForType(typeId, args).pipe(provideDeps))
}

export const propertyRelationValueTypes = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(PropertyResolvers.getPropertyRelationValueTypes(args.id).pipe(provideDeps))
}

export const propertyRenderableType = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(PropertyResolvers.getPropertyRenderableType(args.id).pipe(provideDeps))
}

export const types = async (args: QueryTypesArgs) => {
	return await Effect.runPromise(TypeResolvers.getTypes(args).pipe(provideDeps))
}

export const blocks = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getBlocks(args.id).pipe(provideDeps))
}

export const search = async (args: QuerySearchArgs) => {
	return await Effect.runPromise(SearchResolvers.search(args).pipe(provideDeps))
}

export const properties = async (args: QueryPropertiesArgs) => {
	return await Effect.runPromise(PropertyResolvers.getProperties(args).pipe(provideDeps))
}

export const spaces = async (args: QuerySpacesArgs) => {
	return await Effect.runPromise(SpaceResolvers.getSpaces(args).pipe(provideDeps))
}

export const space = async (id: string) => {
	return await Effect.runPromise(SpaceResolvers.getSpace(id).pipe(provideDeps))
}

export const spaceEntity = async (id: string) => {
	return await Effect.runPromise(SpaceResolvers.getSpaceEntity(id).pipe(provideDeps))
}

export const meta = async () => {
	return await Effect.runPromise(MetaResolvers.getMeta().pipe(provideDeps))
}
