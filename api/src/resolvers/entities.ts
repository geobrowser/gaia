import {Effect} from "effect"
import {Storage} from "../services/storage/storage"

export function getEntities(limit = 100, offset = 0) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.entities.findMany({
				limit,
				offset,
				with: {
					properties: true,
				},
			})

			return result.map((r) => {
				return {
					...r,
					properties: r.properties.map((p) => {
						return {
							...p,
							valueType: mapValueType(p.valueType),
						}
					}),
				}
			})
		})
	})
}

type ValueType = "TEXT" | "NUMBER" | "CHECKBOX" | "URL" | "TIME" | "POINT"

function mapValueType(valueType: string): ValueType {
	switch (valueType) {
		case "1":
			return "TEXT"
		case "2":
			return "NUMBER"
		case "3":
			return "CHECKBOX"
		case "4":
			return "URL"
		case "5":
			return "TIME"
		case "6":
			return "POINT"
		default:
			return "TEXT"
	}
}
