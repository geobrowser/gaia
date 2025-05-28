import {SystemIds} from "@graphprotocol/grc-20"
import {and, eq} from "drizzle-orm"
import {Effect} from "effect"
import type {QueryEntitiesArgs} from "../generated/graphql"
import {values} from "../services/storage/schema"
import {Storage} from "../services/storage/storage"
import {type EntityFilter, buildEntityWhere} from "./filters"

export function getEntities(args: QueryEntitiesArgs) {
	const {filter, limit = 100, offset = 0} = args

	return Effect.gen(function* () {
		const db = yield* Storage

		const whereClauses = filter ? buildEntityWhere(filter as EntityFilter) : undefined

		return yield* db.use(async (client) => {
			const entitiesWithMatchingValue = await client.query.entities.findMany({
				limit: Number(limit),
				offset: Number(offset),
				with: {
					values: {
						where: and(whereClauses),
						columns: {
							propertyId: true,
							value: true,
						},
					},
				},
				where: (entities, {exists}) =>
					exists(
						client
							.select()
							.from(values)
							.where(and(eq(values.entityId, entities.id), whereClauses)),
					),
			})

			return entitiesWithMatchingValue.map((result) => {
				return {
					id: result.id,
					createdAt: result.createdAt,
					createdAtBlock: result.createdAtBlock,
					updatedAt: result.updatedAt,
					updatedAtBlock: result.updatedAtBlock,
					name: result.values.find((p) => p.propertyId === SystemIds.NAME_PROPERTY)?.value,
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
			}
		})
	})
}

export function getEntityName(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const nameProperty = yield* db.use(async (client) => {
			const result = await client.query.properties.findFirst({
				where: (properties, {eq, and}) =>
					and(eq(properties.propertyId, SystemIds.NAME_PROPERTY), eq(properties.entityId, id)),
			})

			return result
		})

		return nameProperty?.value ?? null
	})
}

export function getEntityDescription(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		const nameProperty = yield* db.use(async (client) => {
			const result = await client.query.properties.findFirst({
				where: (properties, {eq, and}) =>
					and(eq(properties.propertyId, SystemIds.DESCRIPTION_PROPERTY), eq(properties.entityId, id)),
			})

			return result
		})

		return nameProperty?.value ?? null
	})
}

export function getValues(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.properties.findMany({
				where: (properties, {eq}) => eq(properties.entityId, id),
			})

			return result
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
				position: relation.position,
				spaceId: relation.spaceId,
			}))
		})
	})
}

export function getEntityTypes(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			const result = await client.query.relations.findMany({
				where: (relations, {eq, and}) =>
					and(eq(relations.fromEntityId, id), eq(relations.typeId, SystemIds.TYPES_PROPERTY)),
				with: {
					toEntity: true,
				},
			})

			return result.map((relation) => ({
				id: relation.toEntity.id,
				createdAt: relation.toEntity.createdAt,
				createdAtBlock: relation.toEntity.createdAtBlock,
				updatedAt: relation.toEntity.updatedAt,
				updatedAtBlock: relation.toEntity.updatedAtBlock,
			}))
		})
	})
}

export function getSpaces(id: string) {
	return Effect.gen(function* () {
		const db = yield* Storage

		return yield* db.use(async (client) => {
			// There's currently some kind of circular dependency or disambiguation
			// issue with drizzle if we try and query properties and relations at
			// the same time using query.entities.findFirst({ with: { properties: true, relations: true } })
			//
			// For now we just query them separately. This avoids joins so might be
			// faster anyway (needs validation).
			const [properties, relations] = await Promise.all([
				client.query.properties.findMany({
					where: (properties, {eq}) => eq(properties.entityId, id),
					columns: {
						spaceId: true,
					},
				}),
				client.query.relations.findMany({
					where: (relations, {eq}) => eq(relations.fromEntityId, id),
					columns: {
						spaceId: true,
					},
				}),
			])

			const propertySpaces = properties.map((p) => p.spaceId)
			const relationSpaces = relations.map((r) => r.spaceId)

			return Array.from(new Set([...propertySpaces, ...relationSpaces]))
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

		// Q: Do we list the other supported value types here? Image, Relation, Place, etc?
	}
}
