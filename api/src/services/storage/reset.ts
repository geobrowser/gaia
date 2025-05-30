import {Effect} from "effect"

import {entities, properties, relations, values} from "../../services/storage/schema"
import {Storage, make} from "../../services/storage/storage"
import {Environment, make as makeEnvironment} from "../environment"

const reset = Effect.gen(function* () {
	const db = yield* Storage

	// const c = yield* db.use(async (client) => await client.delete(ipfsCache).execute())
	const e = yield* db.use(async (client) => await client.delete(entities).execute())
	const v = yield* db.use(async (client) => await client.delete(values).execute())
	const r = yield* db.use(async (client) => await client.delete(relations).execute())
	const p = yield* db.use(async (client) => await client.delete(properties).execute())

	console.log("Results:", {e, p, r, v})
}).pipe(Effect.provideServiceEffect(Storage, make))

Effect.runPromise(reset.pipe(Effect.provideServiceEffect(Environment, makeEnvironment)))
