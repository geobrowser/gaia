import {Effect} from "effect"

import {cursors, editors, entities, members, properties, relations, spaces, values} from "../../services/storage/schema"
import {make, Storage} from "../../services/storage/storage"
import {Environment, make as makeEnvironment} from "../environment"

const reset = Effect.gen(function* () {
	const db = yield* Storage

	// const c = yield* db.use(async (client) => await client.delete(ipfsCache).execute())
	const e = yield* db.use(async (client) => await client.delete(entities).execute())
	const v = yield* db.use(async (client) => await client.delete(values).execute())
	const r = yield* db.use(async (client) => await client.delete(relations).execute())
	const p = yield* db.use(async (client) => await client.delete(properties).execute())
	const s = yield* db.use(async (client) => await client.delete(spaces).execute())
	const ed = yield* db.use(async (client) => await client.delete(editors).execute())
	const m = yield* db.use(async (client) => await client.delete(members).execute())
	const c = yield* db.use(async (client) => await client.delete(cursors).execute())

	console.log("Results:", {e, p, r, v, s, ed, m, c})
}).pipe(Effect.provideServiceEffect(Storage, make))

Effect.runPromise(reset.pipe(Effect.provideServiceEffect(Environment, makeEnvironment)))
