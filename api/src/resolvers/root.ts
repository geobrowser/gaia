import {Effect, Layer} from "effect"
import {getAllEntities} from "./entities"

import type {QueryEntitiesArgs} from "../generated/graphql"
import {Environment, make as makeEnvironment} from "../services/environment"
import {Storage, make as makeStorage} from "../services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

export const entities = async (args: QueryEntitiesArgs) => {
	return await Effect.runPromise(getAllEntities(Number(args.limit), Number(args.offset)).pipe(provideDeps))
}
