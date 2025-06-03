import {Effect, Layer} from "effect"
import type {QueryEntitiesArgs, QueryEntityArgs, QueryTypesArgs} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"
import {Storage, make as makeStorage} from "../services/storage/storage"
import * as EntityResolvers from "./entities"
import * as PropertyResolvers from "./properties"
import * as TypeResolvers from "./types"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
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

export const spaces = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getSpaces(args.id).pipe(provideDeps))
}

export const values = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(EntityResolvers.getValues(args.id, args.spaceId).pipe(provideDeps))
}

export const relations = async (args: QueryEntityArgs & {spaceId?: string | null}) => {
	return await Effect.runPromise(EntityResolvers.getRelations(args.id, args.spaceId).pipe(provideDeps))
}

export const property = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(PropertyResolvers.property(args.id).pipe(provideDeps))
}

export const properties = async (typeId: string, args: QueryTypesArgs) => {
	return await Effect.runPromise(PropertyResolvers.properties(typeId, args).pipe(provideDeps))
}

export const types = async (args: QueryTypesArgs) => {
	return await Effect.runPromise(TypeResolvers.getTypes(args).pipe(provideDeps))
}

export const blocks = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getBlocks(args.id).pipe(provideDeps))
}
