import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import {Storage} from "../services/storage/storage"

export function property(propertyId: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findFirst({
				where: (relations, {eq, and}) =>
					and(eq(relations.fromEntityId, propertyId), eq(relations.typeId, SystemIds.VALUE_TYPE_PROPERTY)),
			})

			if (!result) {
				return null
			}

			return {
				id: propertyId,
				valueType: getValueTypeAsText(result.toEntityId),
			}
		})
	})
}

function getValueTypeAsText(valueTypeId: string | undefined): string | null {
	if (!valueTypeId) {
		return null
	}

	switch (valueTypeId) {
		case SystemIds.TEXT:
			return "TEXT"
		case SystemIds.NUMBER:
			return "NUMBER"
		case SystemIds.CHECKBOX:
			return "CHECKBOX"
		case SystemIds.TIME:
			return "TIME"
		case SystemIds.URL:
			return "URL"
		case SystemIds.POINT:
			return "POINT"
		default:
			return "TEXT"
	}
}
