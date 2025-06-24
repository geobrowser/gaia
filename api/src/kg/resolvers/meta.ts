import {Effect} from "effect"
import type {Meta} from "~/src/generated/graphql"
import {Storage} from "~/src/services/storage/storage"

export function getMeta() {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client): Promise<Meta | null> => {
			const result = await client.query.cursors.findFirst({
				where: (cursors, {eq}) => eq(cursors.id, "kg_indexer"),
			})

			if (!result) {
				return null
			}

			return {
				blockCursor: result.cursor,
				blockNumber: result.blockNumber,
			}
		})
	})
}
