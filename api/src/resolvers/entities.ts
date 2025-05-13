import {SystemIds} from "@graphprotocol/grc-20"
import {Effect} from "effect"
import type {Entity} from "../generated/graphql"
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

			return result.map((result) => {
				return {
					id: result.id,
					createdAt: result.createdAt,
					createdAtBlock: result.createdAtBlock,
					updatedAt: result.updatedAt,
					updatedAtBlock: result.updatedAtBlock,
					name: result.properties.find((p) => p.attributeId === SystemIds.NAME_PROPERTY)?.textValue,
				}
			})
		})
	})
}

export function getEntity(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.entities.findFirst({
				where: (entities, {eq}) => eq(entities.id, id),
				with: {
					properties: true,
				},
			})

			if (!result) {
				return null
			}

			return {
				id: result.id,
				createdAt: result.createdAt,
				createdAtBlock: result.createdAtBlock,
				updatedAt: result.updatedAt,
				updatedAtBlock: result.updatedAtBlock,
				name: result.properties.find((p) => p.attributeId === SystemIds.NAME_PROPERTY)?.textValue,
			}
		})
	})
}

export function getEntityName(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const nameProperty = yield* db.use(async (client) => {
			const result = await client.query.properties.findFirst({
				where: (properties, {eq}) =>
					eq(properties.attributeId, SystemIds.NAME_PROPERTY) && eq(properties.entityId, id),
			})

			return result
		})

		return nameProperty?.textValue || null
	})
}

export function getProperties(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.properties.findMany({
				where: (properties, {eq}) => eq(properties.entityId, id),
			})

			return result.map((p) => ({
				...p,
				valueType: mapValueType(p.valueType),
			}))
		})
	})
}

export function getRelations(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq}) => eq(relations.fromEntityId, id),
			})

			return result.map((relation) => ({
				id: relation.id,
				typeId: relation.typeId,
				fromId: relation.fromEntityId,
				toId: relation.toEntityId,
				index: relation.index,
			}))
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
