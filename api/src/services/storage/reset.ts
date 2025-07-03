import {Effect} from "effect"

import {
	editors,
	entities,
	ipfsCache,
	members,
	meta,
	properties,
	relations,
	spaces,
	values,
} from "../../services/storage/schema"
import {make, Storage} from "../../services/storage/storage"
import {Environment, make as makeEnvironment} from "../environment"

const reset = Effect.gen(function* () {
	const db = yield* Storage

	// Run all deletes in a single transaction
	const results = yield* db.use(async (client) => {
		const result = await client.transaction(async (tx) => {
			// Delete in an order that respects foreign key constraints
			// await tx.delete(ipfsCache).execute()
			const v = await tx.delete(values).execute()
			const r = await tx.delete(relations).execute()
			const ed = await tx.delete(editors).execute()
			const m = await tx.delete(members).execute()
			const p = await tx.delete(properties).execute()
			const e = await tx.delete(entities).execute()
			const s = await tx.delete(spaces).execute()
			const c = await tx.delete(meta).execute()

			return {v, r, ed, m, p, e, s, c}
		})

		return result
	})

	console.log("Transaction complete. Results:", results)
}).pipe(Effect.provideServiceEffect(Storage, make))

Effect.runPromise(reset.pipe(Effect.provideServiceEffect(Environment, makeEnvironment)))
