import {Effect} from "effect"
import type {QueryMembersArgs} from "~/src/generated/graphql"
import {Storage} from "~/src/services/storage/storage"

export function getMembers(args: QueryMembersArgs) {
	const {filter, limit, offset} = args

	return Effect.gen(function* () {
		const db = yield* Storage
	})
}
