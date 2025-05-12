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
	const result = await Effect.runPromise(
		EntityResolvers.getEntities(Number(args.limit), Number(args.offset)).pipe(provideDeps),
	)

	return result
}

export const entity = async (args: QueryEntityArgs) => {
	const result = await Effect.runPromise(EntityResolvers.getEntity(args.id).pipe(provideDeps))

	return result
}
