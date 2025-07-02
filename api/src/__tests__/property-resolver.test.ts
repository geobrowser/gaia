import DataLoader from "dataloader"
import {eq} from "drizzle-orm"
import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {beforeAll, describe, expect, test} from "vitest"
import {DataType} from "../generated/graphql"
import * as PropertyResolvers from "../kg/resolvers/properties"
import {Environment, make as makeEnvironment} from "../services/environment"
import {properties} from "../services/storage/schema"
import {make as makeStorage, Storage} from "../services/storage/storage"
import type {GraphQLContext} from "../types"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer)
const provideDeps = Effect.provide(layers)

// Create a mock GraphQL context with dataloaders for testing
const createMockContext = (): GraphQLContext => {
	return {
		entitiesLoader: new DataLoader(async (ids: readonly string[]) => {
			const storage = Effect.runSync(Storage.pipe(provideDeps))
			const entities = await storage.use(async (client) => {
				return await client.query.entities.findMany({
					where: (entities, {inArray}) => inArray(entities.id, ids),
				})
			})

			const entityMap = new Map(entities.map((e) => [e.id, e]))
			return ids.map((id) => entityMap.get(id) ?? null)
		}),
		entityNamesLoader: new DataLoader(async (ids: readonly string[]) => {
			return ids.map(() => null)
		}),
		entityDescriptionsLoader: new DataLoader(async (ids: readonly string[]) => {
			return ids.map(() => null)
		}),
		entityValuesLoader: new DataLoader(async (keys: readonly any[]) => {
			return keys.map(() => [])
		}),
		entityRelationsLoader: new DataLoader(async (keys: readonly any[]) => {
			return keys.map(() => [])
		}),
		propertiesLoader: new DataLoader(async (ids: readonly string[]) => {
			const storage = Effect.runSync(Storage.pipe(provideDeps))
			const props = await storage.use(async (client) => {
				return await client.query.properties.findMany({
					where: (properties, {inArray}) => inArray(properties.id, ids),
				})
			})

			const propsMap = new Map(props.map((p) => [p.id, p]))
			return ids.map((id) => propsMap.get(id) ?? null)
		}),
		entityBacklinksLoader: new DataLoader(async (keys: readonly any[]) => {
			return keys.map(() => [])
		}),
	}
}

describe("Property Resolver Tests", () => {
	let testPropertyId: string

	beforeAll(async () => {
		// Create a test property
		testPropertyId = uuid()

		await Effect.runPromise(
			Effect.gen(function* () {
				const db = yield* Storage

				yield* db.use(async (client) => {
					await client.insert(properties).values({
						id: testPropertyId,
						type: "Text",
					})
				})
			}).pipe(provideDeps),
		)
	})

	test("should get property by ID", async () => {
		const result = await Effect.runPromise(
			PropertyResolvers.getProperty(testPropertyId, createMockContext()).pipe(provideDeps),
		)

		expect(result).toBeDefined()
		expect(result?.id).toBe(testPropertyId)
		expect(result?.dataType).toBe(DataType.Text)
		expect(result?.renderableType).toBe(null)
	})

	test("should return null property for non-existent ID", async () => {
		const nonExistentId = uuid()

		const result = await Effect.runPromise(
			PropertyResolvers.getProperty(nonExistentId, createMockContext()).pipe(provideDeps),
		)

		expect(result).toBe(null)
	})

	test("should handle different data types", async () => {
		const numberPropertyId = uuid()

		// Create a number property
		await Effect.runPromise(
			Effect.gen(function* () {
				const db = yield* Storage

				yield* db.use(async (client) => {
					await client.insert(properties).values({
						id: numberPropertyId,
						type: "Number",
					})
				})
			}).pipe(provideDeps),
		)

		const result = await Effect.runPromise(
			PropertyResolvers.getProperty(numberPropertyId, createMockContext()).pipe(provideDeps),
		)

		expect(result).toBeDefined()
		expect(result?.id).toBe(numberPropertyId)
		expect(result?.dataType).toBe(DataType.Number)
		expect(result?.renderableType).toBe(null)

		// Clean up
		await Effect.runPromise(
			Effect.gen(function* () {
				const db = yield* Storage

				yield* db.use(async (client) => {
					await client.delete(properties).where(eq(properties.id, numberPropertyId))
				})
			}).pipe(provideDeps),
		)
	})
})
