import {eq} from "drizzle-orm"
import {Effect, Layer} from "effect"
import {v4 as uuid} from "uuid"
import {beforeAll, describe, expect, test} from "vitest"
import {Batching, make as makeBatching} from "~/src/services/storage/dataloaders"
import {DataType} from "../generated/graphql"
import * as PropertyResolvers from "../kg/resolvers/properties"
import {Environment, make as makeEnvironment} from "../services/environment"
import {properties} from "../services/storage/schema"
import {make as makeStorage, Storage} from "../services/storage/storage"

const EnvironmentLayer = Layer.effect(Environment, makeEnvironment)
const StorageLayer = Layer.effect(Storage, makeStorage).pipe(Layer.provide(EnvironmentLayer))
const BatchingLayer = Layer.effect(Batching, makeBatching).pipe(Layer.provide(StorageLayer))
const layers = Layer.mergeAll(EnvironmentLayer, StorageLayer, BatchingLayer)
const provideDeps = Effect.provide(layers)

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
		const result = await Effect.runPromise(PropertyResolvers.getProperty(testPropertyId).pipe(provideDeps))

		expect(result).toBeDefined()
		expect(result?.id).toBe(testPropertyId)
		expect(result?.dataType).toBe(DataType.Text)
		expect(result?.renderableType).toBe(null)
	})

	test("should return null property for non-existent ID", async () => {
		const nonExistentId = uuid()

		const result = await Effect.runPromise(PropertyResolvers.getProperty(nonExistentId).pipe(provideDeps))

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

		const result = await Effect.runPromise(PropertyResolvers.getProperty(numberPropertyId).pipe(provideDeps))

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
