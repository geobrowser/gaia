import {Effect, Layer} from "effect"
import {getAllEntities} from "./entities"

import {Environment, make as makeEnvironment} from "../services/environment"
import {Storage, make as makeStorage} from "../services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)

export const entities = async () => {
	return await Effect.runPromise(getAllEntities().pipe(Effect.provide(layers)))
}
