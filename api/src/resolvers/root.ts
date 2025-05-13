import {Effect, Layer} from "effect"
import type {QueryEntitiesArgs, QueryEntityArgs} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"
import {Storage, make as makeStorage} from "../services/storage/storage"
import * as EntityResolvers from "./entities"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

export const entities = async (args: QueryEntitiesArgs) => {
	return await Effect.runPromise(
		EntityResolvers.getEntities(Number(args.limit), Number(args.offset)).pipe(provideDeps),
	)
}

export const entity = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntity(args.id).pipe(provideDeps))
}

export const entityName = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getEntityName(args.id).pipe(provideDeps))
}

export const types = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getTypes(args.id).pipe(provideDeps))
}

export const spaces = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getSpaces(args.id).pipe(provideDeps))
}

export const properties = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getProperties(args.id).pipe(provideDeps))
}

export const relations = async (args: QueryEntityArgs) => {
	return await Effect.runPromise(EntityResolvers.getRelations(args.id).pipe(provideDeps))
}
