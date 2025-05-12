import {Effect} from "effect"
import {Storage} from "../services/storage/storage"

export function getAllEntities(first = 100, offset = 0) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use((client) =>
			client.query.entities.findMany({
				limit: first,
				offset,
			}),
		)
	})
}
